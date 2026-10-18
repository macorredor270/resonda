// Descarga un archivo por trozos (Range) y en paralelo. YouTube limita mucho las
// peticiones de un solo golpe, pero los trozos de unos MB van a toda velocidad.
//
// Lo que se intenta no hacer nunca: guardar datos equivocados sin avisar. Un trozo solo se acepta
// si el servidor respondió con el rango pedido y llegó completo; los fallos pasajeros (red, 429,
// 5xx) se reintentan con espera, y los definitivos (403, 404...) se cortan al primer intento.

use crate::io::write_all_at;
use crate::{explain, Cancelled, Res};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// Cabeceras HTTP que acompañan a cada petición de descarga (User-Agent, Referer, Cookie...).
pub type Headers = [(String, String)];

/// Petición GET con las cabeceras del servicio.
pub fn get(agent: &ureq::Agent, url: &str, headers: &Headers) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    headers.iter().fold(agent.get(url), |req, (k, v)| req.header(k, v))
}

/// Parámetros de la descarga (las pruebas usan valores pequeños para ir rápido).
#[derive(Clone, Copy)]
pub(crate) struct Tuning {
    pub chunk: u64,
    pub retries: u32,
    pub backoff_ms: u64,
    pub concurrency: usize,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { chunk: 8 * 1024 * 1024, retries: 4, backoff_ms: 400, concurrency: 4 }
    }
}

/// Ningún archivo de vídeo o audio razonable pasa de esto: un tamaño mayor es un dato corrupto.
const MAX_SIZE: u64 = 1 << 40;

/// Un intento fallido: `Retry` si puede arreglarse esperando, `Fatal` si no.
enum Fail {
    Retry(String),
    Fatal(String),
}

/// Duerme `ms` milisegundos pero se despierta si se cancela. Devuelve `true` si se canceló.
fn sleep_unless_cancelled(ms: u64, cancel: &AtomicBool) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        thread::sleep(Duration::from_millis(20).min(end.saturating_duration_since(Instant::now())));
    }
    cancel.load(Ordering::Relaxed)
}

fn attempt(agent: &ureq::Agent, url: &str, headers: &Headers, start: u64, end: u64) -> Result<Vec<u8>, Fail> {
    let want = end - start + 1;
    let resp = get(agent, url, headers)
        .header("Range", format!("bytes={start}-{end}"))
        .call()
        .map_err(|e| Fail::Retry(explain(&e)))?;
    let code = resp.status().as_u16();
    match code {
        206 => {
            // Si dice qué rango manda, tiene que ser el pedido
            let said = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("bytes "));
            if let Some(from) = said.and_then(|v| v.split('-').next()).and_then(|n| n.trim().parse::<u64>().ok()) {
                if from != start {
                    return Err(Fail::Fatal(format!("El servidor mandó otro trozo del que se pidió (empieza en {from}, no en {start})")));
                }
            }
        }
        // 200 solo vale para el primer trozo: el cuerpo empieza en el byte 0. Para cualquier otro,
        // el servidor ha ignorado el rango y guardar esos bytes dejaría el archivo corrupto.
        200 if start == 0 => {}
        200 => return Err(Fail::Fatal("El servidor no admite descargas por trozos (ignoró el rango pedido)".into())),
        403 if url.contains("googlevideo.com") => {
            return Err(Fail::Fatal(format!(
                "YouTube cortó la descarga (HTTP 403 a partir del byte {start}): no entrega el resto de este vídeo a las apps sin sesión iniciada"
            )))
        }
        401 | 403 => {
            return Err(Fail::Fatal(format!("El servidor rechazó la descarga (HTTP {code} a partir del byte {start}): el enlace caducó o el servicio bloquea el acceso")))
        }
        404 | 410 => return Err(Fail::Fatal(format!("El archivo ya no está disponible en el servidor (HTTP {code})"))),
        416 => return Err(Fail::Fatal("El servidor dice que el tamaño del archivo no cuadra (HTTP 416)".into())),
        429 | 500..=599 => return Err(Fail::Retry(format!("el servidor respondió HTTP {code}"))),
        other => return Err(Fail::Fatal(format!("Respuesta inesperada del servidor (HTTP {other})"))),
    }
    let mut buf = Vec::with_capacity(want as usize);
    resp.into_body().into_reader().take(want).read_to_end(&mut buf).map_err(|e| Fail::Retry(explain(&e)))?;
    if buf.len() as u64 != want {
        return Err(Fail::Retry(format!("trozo incompleto: llegaron {} de {want} bytes", buf.len())));
    }
    Ok(buf)
}

fn fetch_range(agent: &ureq::Agent, url: &str, headers: &Headers, (start, end): (u64, u64), t: &Tuning, cancel: &AtomicBool) -> Res<Vec<u8>> {
    let mut last = String::new();
    for n in 1..=t.retries {
        match attempt(agent, url, headers, start, end) {
            Ok(buf) => return Ok(buf),
            Err(Fail::Fatal(msg)) => return Err(msg.into()),
            Err(Fail::Retry(msg)) => last = msg,
        }
        if n < t.retries && sleep_unless_cancelled(t.backoff_ms * u64::from(n * n), cancel) {
            return Err(Box::new(Cancelled));
        }
    }
    Err(format!("Falló la descarga de los bytes {start}-{end} tras {} intentos: {last}", t.retries).into())
}

// Algunos formatos no traen contentLength: se saca de Content-Range pidiendo un solo byte
fn probe_size(agent: &ureq::Agent, url: &str, headers: &Headers, t: &Tuning, cancel: &AtomicBool) -> Res<u64> {
    let mut last = String::new();
    for n in 1..=t.retries {
        match get(agent, url, headers).header("Range", "bytes=0-0").call() {
            Ok(resp) => match resp.status().as_u16() {
                206 => {
                    let total = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit('/').next()).and_then(|n| n.trim().parse::<u64>().ok());
                    return total.filter(|&n| n > 0).ok_or_else(|| "No se pudo averiguar el tamaño del archivo".into());
                }
                200 => return Err("El servidor no admite descargas por trozos y no dice el tamaño del archivo".into()),
                403 | 401 => return Err("El servidor rechazó el acceso al archivo (HTTP 403): el enlace caducó o el servicio lo bloquea".into()),
                404 | 410 => return Err("El archivo ya no está disponible en el servidor".into()),
                429 | 500..=599 => last = format!("el servidor respondió HTTP {}", resp.status().as_u16()),
                other => return Err(format!("Respuesta inesperada del servidor (HTTP {other})").into()),
            },
            Err(e) => last = explain(&e),
        }
        if n < t.retries && sleep_unless_cancelled(t.backoff_ms * u64::from(n * n), cancel) {
            return Err(Box::new(Cancelled));
        }
    }
    Err(format!("No se pudo consultar el archivo tras {} intentos: {last}", t.retries).into())
}

/// Descarga `url` en `dest`. `size` = 0 si no se conoce de antemano.
/// Si `cancel` pasa a true, se detiene y devuelve `Cancelled`.
pub fn download_file(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    size: u64,
    headers: &Headers,
    on_progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Res<()> {
    download_with(agent, url, dest, size, headers, on_progress, cancel, &Tuning::default())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn download_with(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    size: u64,
    headers: &Headers,
    on_progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
    t: &Tuning,
) -> Res<()> {
    let size = if size == 0 { probe_size(agent, url, headers, t, cancel)? } else { size };
    if size == 0 || size > MAX_SIZE {
        return Err(format!("El servidor indica un tamaño de archivo absurdo ({size} bytes)").into());
    }
    let file = File::create(dest).map_err(|e| format!("No se pudo crear {}: {}", dest.display(), explain(&e)))?;
    file.set_len(size).map_err(|e| format!("No se pudo reservar espacio para el archivo: {}", explain(&e)))?;

    let chunks: Vec<(u64, u64)> = (0..size).step_by(t.chunk as usize).map(|s| (s, (s + t.chunk).min(size) - 1)).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicU64::new(0);
    let failure: Mutex<Option<Box<dyn std::error::Error + Send + Sync>>> = Mutex::new(None);
    let fail = |e: Box<dyn std::error::Error + Send + Sync>| {
        failure.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert(e);
    };

    thread::scope(|scope| {
        for _ in 0..t.concurrency.min(chunks.len()) {
            scope.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) {
                    fail(Box::new(Cancelled));
                }
                if failure.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(&range) = chunks.get(i) else { break };
                match fetch_range(agent, url, headers, range, t, cancel) {
                    Ok(buf) => {
                        if let Err(e) = write_all_at(&file, &buf, range.0) {
                            fail(format!("No se pudo escribir en el disco: {}", explain(&e)).into());
                            break;
                        }
                        let total_done = done.fetch_add(buf.len() as u64, Ordering::SeqCst) + buf.len() as u64;
                        on_progress(total_done, size);
                    }
                    Err(e) => {
                        fail(e);
                        break;
                    }
                }
            });
        }
    });
    match failure.into_inner().unwrap_or_else(|p| p.into_inner()) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_agent;
    use crate::testutil::{pattern, FakeServer, Reply};
    use std::sync::atomic::AtomicU64;

    const SMALL: Tuning = Tuning { chunk: 1000, retries: 3, backoff_ms: 5, concurrency: 4 };

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("resonda-dl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn run(server: &FakeServer, size: u64, name: &str, t: &Tuning, cancel: &AtomicBool) -> (Res<()>, std::path::PathBuf) {
        let d = dir(name);
        let dest = d.join("out.bin");
        let r = download_with(&new_agent(), &format!("{}/f", server.url), &dest, size, &[], &|_, _| {}, cancel, t);
        (r, dest)
    }

    #[test]
    fn baja_en_trozos_y_en_orden() {
        let data = pattern(10_500);
        let s = FakeServer::ok(data.clone());
        let progress = AtomicU64::new(0);
        let d = dir("ok");
        let dest = d.join("out.bin");
        download_with(&new_agent(), &format!("{}/f", s.url), &dest, 10_500, &[], &|done, total| {
            assert_eq!(total, 10_500);
            progress.fetch_max(done, Ordering::SeqCst);
        }, &AtomicBool::new(false), &SMALL)
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(progress.load(Ordering::SeqCst), 10_500);
        assert_eq!(s.hits(), 11); // ceil(10500 / 1000)
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn averigua_el_tamano_si_no_se_conoce() {
        let data = pattern(3_333);
        let s = FakeServer::ok(data.clone());
        let (r, dest) = run(&s, 0, "probe", &SMALL, &AtomicBool::new(false));
        r.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[test]
    fn reintenta_los_fallos_pasajeros() {
        let data = pattern(2_500);
        // Las dos primeras peticiones fallan (500 y 429), la tercera ya sale bien
        let s = FakeServer::start(data.clone(), |r| match r.n {
            0 => Reply::Status(500),
            1 => Reply::Status(429),
            _ => Reply::Normal,
        });
        let (r, dest) = run(&s, 2_500, "retry", &Tuning { concurrency: 1, ..SMALL }, &AtomicBool::new(false));
        r.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[test]
    fn un_trozo_cortado_se_reintenta_y_no_se_acepta_a_medias() {
        let data = pattern(1_800);
        let s = FakeServer::start(data.clone(), |r| if r.n == 0 { Reply::Truncate } else { Reply::Normal });
        let (r, dest) = run(&s, 1_800, "trunc", &Tuning { concurrency: 1, ..SMALL }, &AtomicBool::new(false));
        r.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);

        // Si siempre se corta, error claro tras agotar los intentos
        let s = FakeServer::start(data, |_| Reply::Truncate);
        let (r, _) = run(&s, 1_800, "trunc2", &Tuning { concurrency: 1, ..SMALL }, &AtomicBool::new(false));
        let msg = r.unwrap_err().to_string();
        // Según cómo lo vea la librería HTTP: conexión cortada o trozo incompleto
        assert!(msg.contains("tras 3 intentos") && (msg.contains("incompleto") || msg.contains("conexión")), "{msg}");
    }

    #[test]
    fn los_errores_definitivos_no_se_reintentan() {
        for (code, texto) in [(403, "rechazó"), (404, "ya no está"), (410, "ya no está"), (416, "no cuadra")] {
            let s = FakeServer::start(pattern(100), move |_| Reply::Status(code));
            let (r, _) = run(&s, 100, "fatal", &Tuning { concurrency: 1, ..SMALL }, &AtomicBool::new(false));
            let msg = r.unwrap_err().to_string();
            assert!(msg.contains(texto), "{code}: {msg}");
            assert_eq!(s.hits(), 1, "{code} no debe reintentarse");
        }
    }

    #[test]
    fn el_403_de_youtube_se_explica_aparte() {
        let s = FakeServer::start(pattern(100), |_| Reply::Status(403));
        let d = dir("yt403");
        let url = format!("{}/x?host=rr1.googlevideo.com", s.url);
        let e = download_with(&new_agent(), &url, &d.join("o"), 100, &[], &|_, _| {}, &AtomicBool::new(false), &SMALL).unwrap_err();
        assert!(e.to_string().contains("YouTube cortó"), "{e}");
    }

    #[test]
    fn un_servidor_que_ignora_el_rango_no_corrompe_el_archivo() {
        // Con 200 en lugar de 206 el segundo trozo sería el principio del archivo: tiene que fallar
        let s = FakeServer::start(pattern(2_500), |_| Reply::IgnoreRange);
        let (r, _) = run(&s, 2_500, "ignore", &Tuning { concurrency: 1, ..SMALL }, &AtomicBool::new(false));
        assert!(r.unwrap_err().to_string().contains("no admite descargas por trozos"));
    }

    #[test]
    fn un_archivo_de_un_solo_trozo_admite_200() {
        let data = pattern(800);
        let s = FakeServer::start(data.clone(), |_| Reply::IgnoreRange);
        let (r, dest) = run(&s, 800, "single", &SMALL, &AtomicBool::new(false));
        r.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[test]
    fn tamanos_absurdos_o_desconocidos() {
        let s = FakeServer::ok(pattern(100));
        let (r, _) = run(&s, MAX_SIZE + 1, "huge", &SMALL, &AtomicBool::new(false));
        assert!(r.unwrap_err().to_string().contains("absurdo"));
        // Sin tamaño y un servidor que no entiende Range: no se puede seguir
        let s = FakeServer::start(pattern(100), |_| Reply::IgnoreRange);
        let (r, _) = run(&s, 0, "noprobe", &SMALL, &AtomicBool::new(false));
        assert!(r.unwrap_err().to_string().contains("no admite"));
    }

    #[test]
    fn cancelar_corta_la_descarga_y_las_esperas() {
        let s = FakeServer::start(pattern(50_000), |_| Reply::Slow(Duration::from_millis(150)));
        let cancel = AtomicBool::new(false);
        let t0 = Instant::now();
        let (r, _) = thread::scope(|sc| {
            sc.spawn(|| {
                thread::sleep(Duration::from_millis(100));
                cancel.store(true, Ordering::SeqCst);
            });
            run(&s, 50_000, "cancel", &SMALL, &cancel)
        });
        assert!(r.unwrap_err().downcast_ref::<Cancelled>().is_some());
        assert!(t0.elapsed() < Duration::from_secs(3), "tardó {:?}", t0.elapsed());

        // Una espera larga entre reintentos también se interrumpe
        let s = FakeServer::start(pattern(100), |_| Reply::Status(500));
        let cancel = AtomicBool::new(false);
        let t0 = Instant::now();
        let slow = Tuning { backoff_ms: 5_000, ..SMALL };
        let (r, _) = thread::scope(|sc| {
            sc.spawn(|| {
                thread::sleep(Duration::from_millis(100));
                cancel.store(true, Ordering::SeqCst);
            });
            run(&s, 100, "cancel2", &slow, &cancel)
        });
        assert!(r.unwrap_err().downcast_ref::<Cancelled>().is_some());
        assert!(t0.elapsed() < Duration::from_secs(2), "tardó {:?}", t0.elapsed());
    }

    #[test]
    fn destino_imposible_da_un_error_claro() {
        let s = FakeServer::ok(pattern(100));
        let r = download_with(&new_agent(), &format!("{}/f", s.url), Path::new("/no/existe/esta/carpeta/out.bin"), 100, &[], &|_, _| {}, &AtomicBool::new(false), &SMALL);
        assert!(r.unwrap_err().to_string().contains("No se pudo crear"));
    }

    #[test]
    fn sin_servidor_el_error_habla_de_la_conexion() {
        // Puerto cerrado: se reintenta y al final se explica en castellano
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let d = dir("closed");
        let r = download_with(&new_agent(), &format!("http://127.0.0.1:{port}/f"), &d.join("o"), 100, &[], &|_, _| {}, &AtomicBool::new(false), &SMALL);
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("tras 3 intentos"), "{msg}");
    }
}
