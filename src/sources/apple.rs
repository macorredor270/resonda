// Apple Music: metadatos de la API pública de búsqueda de iTunes (`itunes.apple.com/lookup`), sin
// cuenta. El audio de Apple Music tiene DRM y no se toca: se busca y se descarga desde YouTube.
// Canciones y álbumes; las playlists no traen el artista de cada canción en la web pública.

use crate::sources::catalog::{path_segments, query_param, Catalog};
use crate::sources::spotify::{Collection, Kind, Track, BROWSER_UA};
use crate::sources::{on_domain, retry_call};
use crate::Res;
use serde_json::Value;

pub struct AppleMusic;

impl Catalog for AppleMusic {
    fn id(&self) -> &'static str {
        "applemusic"
    }
    fn name(&self) -> &'static str {
        "Apple Music"
    }
    /// El ID lleva la tienda delante (`us/697194953`): los catálogos cambian de un país a otro.
    fn parse(&self, input: &str) -> Option<(Kind, String)> {
        if !on_domain(input, "music.apple.com") {
            return None;
        }
        let segs = path_segments(input);
        let (country, rest) = match segs.split_first() {
            Some((c, rest)) if c.len() == 2 && c.bytes().all(|b| b.is_ascii_lowercase()) => (*c, rest),
            _ => ("us", segs.as_slice()),
        };
        let (&what, last) = (rest.first()?, rest.last()?);
        let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let (kind, id) = match what {
            // Un álbum con `?i=<canción>` es esa canción
            "album" => match query_param(input, "i") {
                Some(song) if numeric(song) => (Kind::Track, song),
                _ => (Kind::Album, *last),
            },
            "song" => (Kind::Track, *last),
            "playlist" => (Kind::Playlist, *last),
            _ => return None,
        };
        (kind == Kind::Playlist || numeric(id)).then(|| (kind, format!("{country}/{id}")))
    }
    fn fetch(&self, agent: &ureq::Agent, kind: Kind, id: &str) -> Res<Collection> {
        if kind == Kind::Playlist {
            return Err("Apple Music: las playlists aún no están soportadas; pega el enlace de un álbum o de una canción".into());
        }
        let (country, id) = id.split_once('/').unwrap_or(("us", id));
        let url = format!("https://itunes.apple.com/lookup?id={id}&entity=song&country={country}");
        let mut resp = retry_call(|| agent.get(&url).header("User-Agent", BROWSER_UA).call())?;
        if !resp.status().is_success() {
            return Err(format!("Apple Music respondió HTTP {}", resp.status().as_u16()).into());
        }
        collection_from(kind, &resp.body_mut().read_json::<Value>()?)
    }
}

/// La portada viene en 100x100: se pide la misma imagen más grande.
fn big_cover(url: &str) -> String {
    url.replace("100x100bb", "600x600bb")
}

pub(crate) fn collection_from(kind: Kind, v: &Value) -> Res<Collection> {
    let results = v["results"].as_array().map(Vec::as_slice).unwrap_or_default();
    let song = |r: &Value, album: Option<&str>| Track {
        id: r["trackId"].as_u64().map(|n| n.to_string()).unwrap_or_default(),
        title: r["trackName"].as_str().unwrap_or("").to_string(),
        artists: r["artistName"].as_str().map(|a| vec![a.to_string()]).unwrap_or_default(),
        duration_ms: r["trackTimeMillis"].as_u64().unwrap_or(0),
        album: album.or(r["collectionName"].as_str()).map(str::to_string),
        year: r["releaseDate"].as_str().map(|d| d.chars().take(4).collect()),
        cover_url: r["artworkUrl100"].as_str().map(big_cover),
    };
    let is_song = |r: &&Value| r["wrapperType"] == "track" && r["kind"] == "song";
    match kind {
        Kind::Track => {
            let t = results.iter().find(is_song).ok_or("Apple Music: no encontré esa canción (¿es de otro país o no es una canción?)")?;
            Ok(Collection { kind, name: t["trackName"].as_str().unwrap_or("").to_string(), tracks: vec![song(t, None)] })
        }
        _ => {
            let album = results.iter().find(|r| r["wrapperType"] == "collection").ok_or("Apple Music: no encontré ese álbum")?;
            let name = album["collectionName"].as_str().unwrap_or("").to_string();
            let tracks: Vec<Track> = results.iter().filter(is_song).map(|r| song(r, Some(&name))).collect();
            if tracks.is_empty() {
                return Err("No se encontraron canciones en ese enlace de Apple Music".into());
            }
            Ok(Collection { kind, name, tracks })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enlaces_de_apple_music() {
        let p = |s: &str| AppleMusic.parse(s);
        assert_eq!(p("https://music.apple.com/us/album/discovery/697194953"), Some((Kind::Album, "us/697194953".into())));
        assert_eq!(p("https://music.apple.com/es/album/one-more-time/697194953?i=697194954"), Some((Kind::Track, "es/697194954".into())));
        assert_eq!(p("https://music.apple.com/gb/song/one-more-time/697194954"), Some((Kind::Track, "gb/697194954".into())));
        assert_eq!(p("https://music.apple.com/album/discovery/697194953"), Some((Kind::Album, "us/697194953".into())));
        assert_eq!(p("https://music.apple.com/gb/playlist/x/pl.0eb1559b2a614029aa9efe486d8ff293").map(|x| x.0), Some(Kind::Playlist));
        assert_eq!(p("https://music.apple.com/us/artist/daft-punk/5468295"), None);
        assert_eq!(p("https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"), None);
    }

    #[test]
    fn respuestas_danadas_no_hacen_panico() {
        let json = r#"{"resultCount":2,"results":[{"wrapperType":"collection","collectionName":"Discovery"},
            {"wrapperType":"track","kind":"song","trackId":1,"trackName":"One","artistName":"Daft","trackTimeMillis":320357,"releaseDate":"2000-11-30T08:00:00Z","artworkUrl100":"https://x/100x100bb.jpg"}]}"#;
        let mut rng = crate::testutil::Rng(14);
        for text in crate::testutil::mutations(json, &mut rng, 3000) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                for kind in [Kind::Track, Kind::Album, Kind::Playlist] {
                    let _ = collection_from(kind, &v);
                }
            }
        }
        for weird in [json!(null), json!({"results":7}), json!({"results":[null, 5, "x", []]}), json!({"results":[{"wrapperType":"track","kind":"song","trackId":"x","trackName":5}]})] {
            let _ = collection_from(Kind::Album, &weird);
            let _ = collection_from(Kind::Track, &weird);
        }
    }

    #[test]
    fn album_y_cancion() {
        let v = json!({"resultCount":3,"results":[
            {"wrapperType":"collection","collectionName":"Discovery","artistName":"Daft Punk"},
            {"wrapperType":"track","kind":"song","trackId":1,"trackName":"One More Time","artistName":"Daft Punk","trackTimeMillis":320357,"collectionName":"Discovery (Deluxe)","releaseDate":"2000-11-30T08:00:00Z","artworkUrl100":"https://x/100x100bb.jpg"},
            {"wrapperType":"track","kind":"music-video","trackId":2,"trackName":"Clip"}]});
        let c = collection_from(Kind::Album, &v).unwrap();
        assert_eq!((c.name.as_str(), c.tracks.len()), ("Discovery", 1));
        let t = &c.tracks[0];
        assert_eq!((t.title.as_str(), t.duration_ms, t.album.as_deref(), t.year.as_deref()), ("One More Time", 320_357, Some("Discovery"), Some("2000")));
        assert_eq!(t.cover_url.as_deref(), Some("https://x/600x600bb.jpg"));
        assert_eq!(collection_from(Kind::Track, &v).unwrap().tracks[0].id, "1");
        assert!(collection_from(Kind::Track, &json!({"results":[]})).is_err());
    }
}
