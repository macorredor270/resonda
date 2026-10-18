//! De dónde salen los datos: un extractor por servicio (YouTube, TikTok...), Spotify (solo
//! metadatos), el emparejamiento entre ambos y la descarga por trozos.
//!
//! Para añadir una red nueva: crea `sources/<red>.rs` con un tipo que implemente [`Extractor`]
//! y añádelo a [`EXTRACTORS`]. El resto (CLI, ventana, formatos de salida) funciona sin tocarlo.

pub mod download;
pub mod matching;
pub mod spotify;
pub mod tiktok;
pub mod youtube;

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
pub static EXTRACTORS: &[&dyn Extractor] = &[&tiktok::TikTok, &youtube::YouTube];

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
    fn cada_enlace_va_a_su_servicio() {
        let id = |s| find(s).map(|e| e.id());
        assert_eq!(id("https://www.tiktok.com/@user/video/7106594312292453675"), Some("tiktok"));
        assert_eq!(id("https://vt.tiktok.com/ZSabc123/"), Some("tiktok"));
        assert_eq!(id("https://youtu.be/dQw4w9WgXcQ"), Some("youtube"));
        assert_eq!(id("dQw4w9WgXcQ"), Some("youtube"));
        assert_eq!(id("https://example.com/video"), None);
    }
}
