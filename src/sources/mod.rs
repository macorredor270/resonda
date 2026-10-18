//! De dónde salen los datos: un extractor por servicio (YouTube, TikTok...), Spotify (solo
//! metadatos), el emparejamiento entre ambos y la descarga por trozos.
//!
//! Para añadir una red nueva: crea `sources/<red>.rs` con un tipo que implemente [`Extractor`]
//! y añádelo a [`EXTRACTORS`]. El resto (CLI, ventana, formatos de salida) funciona sin tocarlo.

pub mod apple;
pub mod bandcamp;
pub mod catalog;
pub mod deezer;
pub mod download;
pub mod matching;
pub mod spotify;
pub mod tidal;
pub mod tiktok;
pub mod twitch;
pub mod youtube;

pub use catalog::{find_catalog, Catalog, CATALOGS};
use crate::sources::youtube::VideoInfo;
use crate::Res;

/// Un servicio del que se pueden bajar vídeos: reconoce sus enlaces y devuelve los formatos.
pub trait Extractor: Sync {
    /// Identificador estable, en minúsculas (`"tiktok"`): lo usan la ventana y los ajustes.
    fn id(&self) -> &'static str;
    /// Nombre para mostrar (`"TikTok"`).
    fn name(&self) -> &'static str;
    /// ¿Es este enlace (o ID) de este servicio? No debe usar la red.
    fn matches(&self, input: &str) -> bool;
    /// Consulta el enlace y devuelve título, duración y formatos descargables.
    fn extract(&self, agent: &ureq::Agent, input: &str) -> Res<VideoInfo>;
}

/// Servicios soportados, por orden de prueba.
pub static EXTRACTORS: &[&dyn Extractor] = &[&tiktok::TikTok, &bandcamp::Bandcamp, &twitch::Twitch, &youtube::YouTube];

/// El extractor que reconoce `input`, si hay alguno.
pub fn find(input: &str) -> Option<&'static dyn Extractor> {
    EXTRACTORS.iter().copied().find(|e| e.matches(input.trim()))
}

/// Consulta un enlace con el extractor que le corresponda.
pub fn fetch_info(agent: &ureq::Agent, input: &str) -> Res<VideoInfo> {
    let input = input.trim();
    match find(input) {
        Some(e) => e.extract(agent, input),
        None => {
            let names: Vec<_> = EXTRACTORS.iter().map(|e| e.name()).collect();
            Err(format!("\"{input}\" no es un enlace de un servicio soportado ({})", names.join(", ")).into())
        }
    }
}

type Resp = ureq::http::Response<ureq::Body>;

/// Hace una petición y la repite (hasta 3 veces, con espera creciente) si falla la red o el servidor
/// responde 429 o 5xx. Cualquier otro código se devuelve tal cual para que lo interprete quien llama.
pub fn retry_call(f: impl FnMut() -> Result<Resp, ureq::Error>) -> Res<Resp> {
    retry_call_with(300, f)
}

pub(crate) fn retry_call_with(backoff_ms: u64, mut f: impl FnMut() -> Result<Resp, ureq::Error>) -> Res<Resp> {
    let mut last = None;
    for n in 1..=3u64 {
        match f() {
            Ok(r) if !matches!(r.status().as_u16(), 429 | 500..=599) => return Ok(r),
            r => last = Some(r),
        }
        if n < 3 {
            std::thread::sleep(std::time::Duration::from_millis(backoff_ms * n * n));
        }
    }
    Ok(last.expect("al menos un intento")?)
}

/// Servidor de un enlace en minúsculas y sin `www.` (`None` si no parece una URL).
pub fn host_of(input: &str) -> Option<String> {
    let rest = input.split_once("://").map_or(input, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?.split(':').next()?;
    let host = host.trim_start_matches("www.").to_ascii_lowercase();
    (host.contains('.') && !host.contains(char::is_whitespace)).then_some(host)
}

/// ¿Está el servidor de `input` en `domain` o es un subdominio suyo?
pub fn on_domain(input: &str, domain: &str) -> bool {
    host_of(input).is_some_and(|h| h == domain || h.ends_with(&format!(".{domain}")))
}

/// Codifica un valor para una URL (`%XX` para todo menos letras, cifras y `-_.~`).
pub fn percent_encode(s: &str) -> String {
    s.bytes().fold(String::with_capacity(s.len() * 2), |mut out, b| {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        out
    })
}

/// Deshace los escapes HTML más comunes (`&quot;`, `&amp;`, `&#39;`, `&#x27;`...).
pub fn html_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&end| end <= 10).and_then(|end| {
            let ch = match &rest[1..end] {
                "quot" => '"',
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "apos" => '\'',
                e if e.starts_with("#x") => char::from_u32(u32::from_str_radix(&e[2..], 16).ok()?)?,
                e if e.starts_with('#') => char::from_u32(e[1..].parse().ok()?)?,
                _ => return None,
            };
            Some((ch, end + 1))
        });
        match decoded {
            Some((ch, len)) => {
                out.push(ch);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Cookies que un servicio reparte a cualquier visitante, listas para la cabecera `Cookie`.
pub fn cookie_header(headers: &ureq::http::HeaderMap) -> String {
    let mut jar: Vec<(String, String)> = Vec::new();
    for v in headers.get_all("set-cookie") {
        let Some((name, value)) = v.to_str().ok().and_then(|s| s.split(';').next()).and_then(|kv| kv.split_once('=')) else {
            continue;
        };
        jar.retain(|(n, _)| n != name.trim());
        jar.push((name.trim().to_string(), value.to_string()));
    }
    jar.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{pattern, FakeServer, Reply};

    #[test]
    fn retry_call_reintenta_5xx_y_errores_de_red() {
        let agent = crate::new_agent();
        let s = FakeServer::start(pattern(10), |r| if r.n < 2 { Reply::Status(503) } else { Reply::Normal });
        let r = retry_call_with(1, || agent.get(format!("{}/x", s.url)).call()).unwrap();
        assert_eq!((r.status().as_u16(), s.hits()), (200, 3));

        // Si no se arregla, se devuelve el último código para que quien llama lo explique
        let s = FakeServer::start(pattern(10), |_| Reply::Status(500));
        let r = retry_call_with(1, || agent.get(format!("{}/x", s.url)).call()).unwrap();
        assert_eq!((r.status().as_u16(), s.hits()), (500, 3));

        // Los códigos definitivos no se repiten
        let s = FakeServer::start(pattern(10), |_| Reply::Status(404));
        let r = retry_call_with(1, || agent.get(format!("{}/x", s.url)).call()).unwrap();
        assert_eq!((r.status().as_u16(), s.hits()), (404, 1));

        // Sin servidor: error de red, tras los 3 intentos
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let n = std::cell::Cell::new(0);
        let e = retry_call_with(1, || {
            n.set(n.get() + 1);
            agent.get(format!("http://127.0.0.1:{port}/")).call()
        });
        assert!(e.is_err() && n.get() == 3);
    }

    #[test]
    fn servidores() {
        assert_eq!(host_of("https://www.TikTok.com/@a/video/1?x=1").as_deref(), Some("tiktok.com"));
        assert_eq!(host_of("vm.tiktok.com/ZM123/").as_deref(), Some("vm.tiktok.com"));
        assert_eq!(host_of("dQw4w9WgXcQ"), None);
        assert!(on_domain("https://vm.tiktok.com/abc", "tiktok.com"));
        assert!(!on_domain("https://nottiktok.com/abc", "tiktok.com"));
        assert!(!on_domain("https://evil.com/tiktok.com", "tiktok.com"));
    }

    #[test]
    fn enlaces_basura_no_hacen_panico() {
        use crate::testutil::Rng;
        const PIEZAS: &[&str] = &[
            "https://", "http://", "www.", "m.", "music.", "youtube.com", "youtu.be", "tiktok.com", "vm.tiktok.com", "twitch.tv", "clips.twitch.tv",
            "bandcamp.com", "deezer.com", "music.apple.com", "tidal.com", "open.spotify.com", "spotify:", "/", "//", "?", "&", "=", "#", "@", ":", ";",
            "video", "clip", "track", "album", "playlist", "song", "watch", "v=", "i=", "embed", "en", "us", "pt-br", "browse", "é", "日本", "\u{0}",
            "%20", " ", "..", "dQw4w9WgXcQ", "12345", "pl.abc", "99999999999999999999999", "-", "_", "\t", "\n",
        ];
        let mut rng = Rng(0xc0ff_ee11);
        for _ in 0..30_000 {
            let s: String = (0..1 + rng.below(14)).map(|_| *rng.pick(PIEZAS)).collect();
            let _ = (host_of(&s), on_domain(&s, "tiktok.com"), html_unescape(&s), percent_encode(&s), crate::pipeline::safe_name(&s));
            let _ = (find(&s).map(|e| e.id()), find_catalog(&s).map(|c| c.0.id()), youtube::parse_video_id(&s), twitch::clip_slug(&s));
            for e in EXTRACTORS {
                let _ = e.matches(&s);
            }
            for c in CATALOGS {
                let _ = c.parse(&s);
            }
            let _ = (catalog::path_segments(&s), catalog::query_param(&s, "i"), catalog::iso_duration_ms(&s), catalog::ld_json(&s));
        }
    }

    #[test]
    fn un_enlace_solo_lo_reconoce_un_servicio() {
        // Ningún enlace de los ejemplos cae en dos servicios a la vez
        let enlaces = [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "https://youtu.be/dQw4w9WgXcQ", "https://www.tiktok.com/@a/video/1", "https://vm.tiktok.com/ZM1/",
            "https://clips.twitch.tv/AbCd-123", "https://banditcamp.bandcamp.com/track/track-i", "https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT",
            "https://www.deezer.com/en/track/3135556", "https://music.apple.com/us/album/discovery/697194953", "https://tidal.com/browse/track/1493005",
        ];
        for e in enlaces {
            let hits = EXTRACTORS.iter().filter(|x| x.matches(e)).count() + CATALOGS.iter().filter(|c| c.parse(e).is_some()).count();
            assert_eq!(hits, 1, "{e} lo reconocen {hits} servicios");
        }
    }

    #[test]
    fn codifica_urls() {
        assert_eq!(percent_encode("a b/ñ{\"x\":1}"), "a%20b%2F%C3%B1%7B%22x%22%3A1%7D");
    }

    #[test]
    fn escapes_html() {
        assert_eq!(html_unescape("a &quot;b&quot; &amp; c &#39;d&#x27; &lt;e&gt; & f &nada; &#xZZ;"), "a \"b\" & c 'd' <e> & f &nada; &#xZZ;");
        assert_eq!(html_unescape("sin escapes"), "sin escapes");
    }

    #[test]
    fn cada_enlace_va_a_su_servicio() {
        assert_eq!(find("https://clips.twitch.tv/AbCd-123").map(|e| e.id()), Some("twitch"));
        assert_eq!(find("https://banditcamp.bandcamp.com/track/track-i").map(|e| e.id()), Some("bandcamp"));
        let id = |s| find(s).map(|e| e.id());
        assert_eq!(id("https://www.tiktok.com/@user/video/7106594312292453675"), Some("tiktok"));
        assert_eq!(id("https://vt.tiktok.com/ZSabc123/"), Some("tiktok"));
        assert_eq!(id("https://youtu.be/dQw4w9WgXcQ"), Some("youtube"));
        assert_eq!(id("dQw4w9WgXcQ"), Some("youtube"));
        assert_eq!(id("https://example.com/video"), None);
    }
}
