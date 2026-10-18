//! Catálogos de música (Spotify, Deezer, Apple Music, Tidal): de ellos solo se leen los METADATOS
//! (título, artistas, duración, portada). El audio de esos servicios está protegido con DRM y no se
//! toca: cada canción se busca y se descarga desde YouTube (ver `matching`).
//!
//! Para añadir un catálogo: crea `sources/<nombre>.rs` con un tipo que implemente [`Catalog`] y
//! añádelo a [`CATALOGS`].

use crate::sources::spotify::{Collection, Kind, Track};
use crate::sources::{apple, deezer, spotify, tidal};
use crate::Res;
use serde_json::Value;

pub trait Catalog: Sync {
    /// Identificador estable, en minúsculas (`"deezer"`).
    fn id(&self) -> &'static str;
    /// Nombre para mostrar (`"Deezer"`).
    fn name(&self) -> &'static str;
    /// Reconoce un enlace y devuelve de qué tipo es y su identificador. No usa la red.
    fn parse(&self, input: &str) -> Option<(Kind, String)>;
    /// Lee una canción, un álbum o una playlist.
    fn fetch(&self, agent: &ureq::Agent, kind: Kind, id: &str) -> Res<Collection>;
    /// Datos más precisos de una canción de una lista, si el catálogo los da por separado.
    fn refine(&self, _agent: &ureq::Agent, track: &Track) -> Track {
        track.clone()
    }
}

/// Catálogos soportados, por orden de prueba.
pub static CATALOGS: &[&dyn Catalog] = &[&spotify::Spotify, &deezer::Deezer, &apple::AppleMusic, &tidal::Tidal];

/// El catálogo al que pertenece `input`, con el tipo y el ID que se leyeron del enlace.
pub fn find_catalog(input: &str) -> Option<(&'static dyn Catalog, Kind, String)> {
    let input = input.trim();
    CATALOGS.iter().copied().find_map(|c| c.parse(input).map(|(kind, id)| (c, kind, id)))
}

/// Pasa por `serde_json` todos los bloques `application/ld+json` de una página.
pub(crate) fn ld_json(html: &str) -> Vec<Value> {
    html.split("application/ld+json").skip(1).filter_map(|rest| {
        let body = rest.split_once('>')?.1.split_once("</script>")?.0;
        serde_json::from_str(body).ok()
    }).collect()
}

/// Duración ISO 8601 (`PT5M20S`, `PT1H2M3.5S`) en milisegundos.
pub(crate) fn iso_duration_ms(s: &str) -> Option<u64> {
    let mut rest = s.strip_prefix("PT")?;
    let mut total = 0f64;
    while !rest.is_empty() {
        let end = rest.find(|c: char| c.is_ascii_alphabetic())?;
        let n: f64 = rest[..end].parse().ok()?;
        total += n * match &rest[end..=end] {
            "H" => 3_600_000.0,
            "M" => 60_000.0,
            "S" => 1000.0,
            _ => return None,
        };
        rest = &rest[end + 1..];
    }
    Some(total.round() as u64)
}

/// Segmentos de la ruta de un enlace, sin parámetros ni `#`.
pub(crate) fn path_segments(input: &str) -> Vec<&str> {
    let rest = input.split_once("://").map_or(input, |(_, r)| r);
    let path = rest.split_once('/').map_or("", |(_, p)| p);
    path.split(['?', '#']).next().unwrap_or("").split('/').filter(|s| !s.is_empty()).collect()
}

/// Un parámetro de la consulta (`?i=123`).
pub(crate) fn query_param<'a>(input: &'a str, name: &str) -> Option<&'a str> {
    let query = input.split_once('?')?.1.split('#').next()?;
    query.split('&').find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duraciones_iso() {
        assert_eq!(iso_duration_ms("PT5M20S"), Some(320_000));
        assert_eq!(iso_duration_ms("PT1H2M3.5S"), Some(3_723_500));
        assert_eq!(iso_duration_ms("PT45S"), Some(45_000));
        assert_eq!(iso_duration_ms("5:20"), None);
    }

    #[test]
    fn ayudas_de_enlaces() {
        assert_eq!(path_segments("https://x.com/a/b/?q=1#z"), ["a", "b"]);
        assert_eq!(query_param("https://x.com/a?i=12&b=3", "i"), Some("12"));
        assert_eq!(query_param("https://x.com/a", "i"), None);
    }

    #[test]
    fn cada_catalogo_reconoce_lo_suyo() {
        let id = |s| find_catalog(s).map(|(c, k, i)| (c.id(), k, i));
        assert_eq!(id("https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT").map(|x| x.0), Some("spotify"));
        assert_eq!(id("https://www.deezer.com/en/track/3135556"), Some(("deezer", Kind::Track, "3135556".into())));
        assert_eq!(id("https://music.apple.com/us/album/discovery/697194953"), Some(("applemusic", Kind::Album, "us/697194953".into())));
        assert_eq!(id("https://tidal.com/browse/track/1493005"), Some(("tidal", Kind::Track, "1493005".into())));
        assert_eq!(id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), None);
    }
}
