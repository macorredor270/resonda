//! Servidor HTTP falso para las pruebas: sirve unos bytes con soporte de `Range` y, si se le pide,
//! se porta mal (errores, respuestas cortadas, ignorar el rango, lentitud) para probar los fallos.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum Reply {
    /// Respuesta correcta: 206 con el rango pedido, o 200 con todo si no hay `Range`
    Normal,
    /// Responde solo con este código y sin cuerpo
    Status(u16),
    /// Responde 200 con el archivo entero aunque se pida un rango
    IgnoreRange,
    /// Anuncia el tamaño completo pero corta la conexión a la mitad del cuerpo
    Truncate,
    /// Espera este tiempo y luego responde con normalidad
    Slow(Duration),
}

#[derive(Clone, Debug)]
pub struct Request {
    /// Número de petición (0, 1, 2...) desde que arrancó el servidor
    pub n: usize,
    #[allow(dead_code)]
    pub path: String,
    pub range: Option<(u64, u64)>,
}

pub struct FakeServer {
    pub url: String,
    log: Arc<Mutex<Vec<Request>>>,
}

impl FakeServer {
    pub fn start(data: Vec<u8>, script: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (log, counter, data, script) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)), Arc::new(data), Arc::new(script));
        let server_log = log.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, counter, data, script) = (server_log.clone(), counter.clone(), data.clone(), script.clone());
                thread::spawn(move || {
                    let _ = handle(stream, &data, &*script, &log, &counter);
                });
            }
        });
        FakeServer { url, log }
    }

    /// Un servidor que siempre responde bien.
    pub fn ok(data: Vec<u8>) -> FakeServer {
        FakeServer::start(data, |_| Reply::Normal)
    }

    #[allow(dead_code)]
    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().clone()
    }

    pub fn hits(&self) -> usize {
        self.log.lock().unwrap().len()
    }
}

/// Bytes de prueba reconocibles: cada posición lleva un valor distinto, así un trozo mal colocado se nota.
pub fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 + i / 251) as u8).collect()
}

fn handle(
    mut stream: TcpStream,
    data: &[u8],
    script: &dyn Fn(&Request) -> Reply,
    log: &Mutex<Vec<Request>>,
    counter: &AtomicUsize,
) -> std::io::Result<()> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            return Ok(());
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
    let range = text.lines().find_map(|l| {
        let v = l.to_ascii_lowercase().strip_prefix("range: bytes=")?.to_string();
        let (a, b) = v.trim().split_once('-')?;
        Some((a.parse().ok()?, b.parse().unwrap_or(data.len() as u64 - 1)))
    });
    let req = Request { n: counter.fetch_add(1, Ordering::SeqCst), path, range };
    log.lock().unwrap().push(req.clone());

    let mut reply = script(&req);
    if let Reply::Slow(d) = reply {
        thread::sleep(d);
        reply = Reply::Normal;
    }
    let total = data.len() as u64;
    let send = |stream: &mut TcpStream, status: &str, extra: &str, body: &[u8], claimed: usize| -> std::io::Result<()> {
        write!(stream, "HTTP/1.1 {status}\r\n{extra}Content-Length: {claimed}\r\nConnection: close\r\n\r\n")?;
        stream.write_all(body)?;
        stream.flush()
    };
    match (reply, req.range) {
        (Reply::Status(code), _) => send(&mut stream, &format!("{code} X"), "", b"", 0),
        (Reply::IgnoreRange, _) | (Reply::Normal, None) => send(&mut stream, "200 OK", "", data, data.len()),
        (Reply::Normal, Some((a, b))) => {
            let b = b.min(total.saturating_sub(1));
            if a > b {
                return send(&mut stream, "416 Range Not Satisfiable", "", b"", 0);
            }
            let body = &data[a as usize..=b as usize];
            send(&mut stream, "206 Partial Content", &format!("Content-Range: bytes {a}-{b}/{total}\r\n"), body, body.len())
        }
        (Reply::Truncate, range) => {
            let (a, b) = range.unwrap_or((0, total - 1));
            let b = b.min(total.saturating_sub(1));
            let body = &data[a as usize..=b as usize];
            send(&mut stream, "206 Partial Content", &format!("Content-Range: bytes {a}-{b}/{total}\r\n"), &body[..body.len() / 2], body.len())
        }
        (Reply::Slow(_), _) => unreachable!(),
    }
}

/// Generador pseudoaleatorio mínimo y determinista (xorshift): las pruebas de entradas basura
/// tienen que dar siempre lo mismo para poder reproducir un fallo.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Variantes dañadas de un texto: recortes, bytes cambiados, trozos borrados o repetidos y cambios
/// de comillas y llaves. Sirven para comprobar que un lector de páginas o JSON da error y no pánico.
pub fn mutations(base: &str, rng: &mut Rng, n: usize) -> Vec<String> {
    let bytes = base.as_bytes();
    let mut out = vec![String::new(), base.to_string()];
    out.extend((0..bytes.len()).step_by((bytes.len() / 60).max(1)).map(|i| String::from_utf8_lossy(&bytes[..i]).to_string()));
    for k in 0..n {
        let mut b = bytes.to_vec();
        let at = rng.below(b.len());
        match k % 5 {
            0 => b[at] = rng.next() as u8,
            1 => {
                let end = (at + 1 + rng.below(40)).min(b.len());
                b.drain(at..end);
            }
            2 => {
                let end = (at + 1 + rng.below(40)).min(b.len());
                let piece = b[at..end].to_vec();
                b.splice(at..at, piece);
            }
            3 => b[at] = *rng.pick(b"\"{}[],:<>&;"),
            _ => b.truncate(at),
        }
        out.push(String::from_utf8_lossy(&b).to_string());
    }
    out
}
