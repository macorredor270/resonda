// Tidal: metadatos de la ficha pública de cada canción (datos estructurados JSON-LD de la web),
// sin cuenta. El audio de Tidal tiene DRM y no se toca: se busca y se descarga desde YouTube.
// Solo canciones sueltas: los álbumes y las playlists no muestran su lista sin iniciar sesión.

use crate::sources::catalog::{iso_duration_ms, ld_json, path_segments, Catalog};
use crate::sources::spotify::{Collection, Kind, Track, BROWSER_UA};
use crate::sources::{on_domain, retry_call};
use crate::Res;
use serde_json::Value;

pub struct Tidal;

impl Catalog for Tidal {
    fn id(&self) -> &'static str {
        "tidal"
    }
    fn name(&self) -> &'static str {
        "Tidal"
    }
    fn parse(&self, input: &str) -> Option<(Kind, String)> {
        if !on_domain(input, "tidal.com") {
            return None;
        }
        // tidal.com/[browse/]track|album|playlist/<id>
        let mut segs = path_segments(input).into_iter().skip_while(|s| matches!(*s, "browse" | "u") || s.len() == 2);
        let kind = match segs.next()? {
            "track" => Kind::Track,
            "album" => Kind::Album,
            "playlist" => Kind::Playlist,
            _ => return None,
        };
        let id = segs.next()?;
        let ok = match kind {
            Kind::Playlist => id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
            _ => !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
        };
        ok.then(|| (kind, id.to_string()))
    }
    fn fetch(&self, agent: &ureq::Agent, kind: Kind, id: &str) -> Res<Collection> {
        if kind != Kind::Track {
            return Err("Tidal: solo se pueden leer canciones sueltas (los álbumes y las playlists no muestran su lista sin iniciar sesión); pega el enlace de una canción".into());
        }
        let url = format!("https://tidal.com/browse/track/{id}");
        let mut resp = retry_call(|| agent.get(&url).header("User-Agent", BROWSER_UA).call())?;
        if !resp.status().is_success() {
            return Err(format!("Tidal respondió HTTP {} (¿la canción no existe?)", resp.status().as_u16()).into());
        }
        track_from_html(&resp.body_mut().read_to_string()?, id)
    }
}

pub(crate) fn track_from_html(html: &str, id: &str) -> Res<Collection> {
    let blocks = ld_json(html);
    let t = blocks.iter().find(|b| b["@type"] == "MusicRecording").ok_or("Tidal no devolvió la ficha de la canción (¿no existe o cambió la página?)")?;
    let title = t["name"].as_str().ok_or("Tidal: canción sin título")?.to_string();
    let names = |v: &Value| -> Vec<String> {
        match v {
            Value::Array(a) => a.iter().filter_map(|x| x["name"].as_str().map(str::to_string)).collect(),
            Value::Object(_) => v["name"].as_str().map(str::to_string).into_iter().collect(),
            _ => vec![],
        }
    };
    let track = Track {
        id: id.to_string(),
        title: title.clone(),
        artists: names(&t["byArtist"]),
        duration_ms: t["duration"].as_str().and_then(iso_duration_ms).unwrap_or(0),
        album: t["inAlbum"]["name"].as_str().map(str::to_string),
        year: t["datePublished"].as_str().or(t["inAlbum"]["datePublished"].as_str()).map(|d| d.chars().take(4).collect()),
        cover_url: t["image"].as_str().map(str::to_string),
    };
    if track.artists.is_empty() {
        return Err("Tidal: la canción no indica su artista".into());
    }
    Ok(Collection { kind: Kind::Track, name: title, tracks: vec![track] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enlaces_de_tidal() {
        let p = |s: &str| Tidal.parse(s);
        for u in ["https://tidal.com/browse/track/1493005", "https://tidal.com/track/1493005?u", "https://listen.tidal.com/track/1493005", "https://tidal.com/us/track/1493005"] {
            assert_eq!(p(u), Some((Kind::Track, "1493005".into())), "{u}");
        }
        assert_eq!(p("https://tidal.com/album/1493003").map(|x| x.0), Some(Kind::Album));
        assert_eq!(p("https://tidal.com/browse/playlist/299887f1-3cab-437a-82f6-605e62381477").map(|x| x.0), Some(Kind::Playlist));
        assert_eq!(p("https://tidal.com/browse/artist/8847"), None);
        assert_eq!(p("https://tidal.com/browse/track/abc"), None);
    }

    #[test]
    fn paginas_danadas_no_hacen_panico() {
        let html = r#"<script type="application/ld+json">{"@type":"MusicRecording","name":"One More Time","duration":"PT5M20S","byArtist":[{"name":"Daft Punk"}],"image":"https://x/640.jpg","inAlbum":{"name":"A"}}</script>"#;
        let mut rng = crate::testutil::Rng(15);
        for h in crate::testutil::mutations(html, &mut rng, 3000) {
            let _ = track_from_html(&h, "1");
        }
        for weird in [r#"{"@type":"MusicRecording"}"#, r#"{"@type":"MusicRecording","name":5,"byArtist":7}"#, r#"[]"#, r#"{"@type":"MusicRecording","name":"x","byArtist":"Daft","duration":"PT"}"#, r#"{"@type":"MusicRecording","name":"x","byArtist":{"name":"A"},"duration":"PT99999999999999999999S"}"#] {
            let _ = track_from_html(&format!("<script type=\"application/ld+json\">{weird}</script>"), "1");
        }
    }

    #[test]
    fn ficha_de_cancion() {
        let html = r#"<script type="application/ld+json">{"@type":"MusicRecording","name":"One More Time","duration":"PT5M20S",
            "byArtist":[{"@type":"MusicGroup","name":"Daft Punk"}],"image":"https://x/640.jpg","inAlbum":{"name":"One More Time"}}</script>"#;
        let c = track_from_html(html, "1493005").unwrap();
        let t = &c.tracks[0];
        assert_eq!((t.title.as_str(), t.duration_ms, t.artists.as_slice(), t.album.as_deref()), ("One More Time", 320_000, ["Daft Punk".to_string()].as_slice(), Some("One More Time")));
        assert!(track_from_html("<html></html>", "1").is_err());
    }
}
