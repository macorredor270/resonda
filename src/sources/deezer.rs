// Deezer: metadatos de su API pública (api.deezer.com), sin cuenta. El audio de Deezer tiene DRM y
// no se toca: se busca y se descarga desde YouTube.

use crate::sources::catalog::{path_segments, Catalog};
use crate::sources::spotify::{Collection, Kind, Track};
use crate::sources::{on_domain, spotify};
use crate::Res;
use serde_json::Value;

pub struct Deezer;

impl Catalog for Deezer {
    fn id(&self) -> &'static str {
        "deezer"
    }
    fn name(&self) -> &'static str {
        "Deezer"
    }
    fn parse(&self, input: &str) -> Option<(Kind, String)> {
        if !on_domain(input, "deezer.com") {
            return None;
        }
        // deezer.com/[idioma/]track|album|playlist/<id>
        let mut segs = path_segments(input).into_iter().skip_while(|s| s.len() == 2 || s.contains('-') && s.len() <= 5);
        let kind = match segs.next()? {
            "track" => Kind::Track,
            "album" => Kind::Album,
            "playlist" => Kind::Playlist,
            _ => return None,
        };
        let id = segs.next()?;
        (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| (kind, id.to_string()))
    }
    fn fetch(&self, agent: &ureq::Agent, kind: Kind, id: &str) -> Res<Collection> {
        let path = match kind {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Playlist => "playlist",
        };
        let head = get(agent, &format!("https://api.deezer.com/{path}/{id}"))?;
        if kind == Kind::Track {
            return Ok(Collection { kind, name: head["title"].as_str().unwrap_or("").to_string(), tracks: vec![track_of(&head, None, None)] });
        }
        let name = head["title"].as_str().unwrap_or("").to_string();
        let cover = ["cover_xl", "picture_xl", "cover_big", "picture_big"].iter().find_map(|k| head[*k].as_str()).map(str::to_string);
        let year = head["release_date"].as_str().map(|d| d.chars().take(4).collect::<String>());
        let album = (kind == Kind::Album).then(|| name.clone());

        // Las listas largas vienen por páginas
        let mut tracks = Vec::new();
        let mut url = Some(format!("https://api.deezer.com/{path}/{id}/tracks?limit=100"));
        while let Some(u) = url.take() {
            let page = get(agent, &u)?;
            for t in page["data"].as_array().into_iter().flatten() {
                let mut tr = track_of(t, album.as_deref(), year.as_deref());
                if tr.cover_url.is_none() || kind == Kind::Album {
                    tr.cover_url = cover.clone().or(tr.cover_url);
                }
                tracks.push(tr);
            }
            url = page["next"].as_str().map(str::to_string).filter(|_| tracks.len() < 2000);
        }
        if tracks.is_empty() {
            return Err("No se encontraron canciones en ese enlace de Deezer".into());
        }
        Ok(Collection { kind, name, tracks })
    }
    fn refine(&self, agent: &ureq::Agent, track: &Track) -> Track {
        // Las canciones de una playlist traen el artista principal; la ficha completa tiene a todos
        match self.fetch(agent, Kind::Track, &track.id) {
            Ok(c) => Track { artists: c.tracks[0].artists.clone(), year: c.tracks[0].year.clone().or_else(|| track.year.clone()), ..track.clone() },
            Err(_) => track.clone(),
        }
    }
}

fn get(agent: &ureq::Agent, url: &str) -> Res<Value> {
    let mut resp = agent.get(url).header("User-Agent", spotify::BROWSER_UA).call()?;
    if !resp.status().is_success() {
        return Err(format!("Deezer respondió HTTP {}", resp.status().as_u16()).into());
    }
    let v: Value = resp.body_mut().read_json()?;
    if v["error"].is_object() {
        return Err(format!("Deezer: {}", v["error"]["message"].as_str().unwrap_or("enlace inexistente o privado")).into());
    }
    Ok(v)
}

fn track_of(t: &Value, album: Option<&str>, year: Option<&str>) -> Track {
    let mut artists: Vec<String> = t["contributors"].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str().map(str::to_string)).collect();
    if artists.is_empty() {
        artists.extend(t["artist"]["name"].as_str().map(str::to_string));
    }
    Track {
        id: t["id"].as_u64().map(|n| n.to_string()).unwrap_or_default(),
        title: t["title"].as_str().unwrap_or("").to_string(),
        artists,
        duration_ms: t["duration"].as_u64().unwrap_or(0) * 1000,
        album: album.map(str::to_string).or_else(|| t["album"]["title"].as_str().map(str::to_string)),
        year: year.map(str::to_string).or_else(|| t["release_date"].as_str().map(|d| d.chars().take(4).collect())),
        cover_url: ["cover_xl", "cover_big"].iter().find_map(|k| t["album"][*k].as_str()).map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enlaces_de_deezer() {
        let p = |s: &str| Deezer.parse(s);
        assert_eq!(p("https://www.deezer.com/en/track/3135556"), Some((Kind::Track, "3135556".into())));
        assert_eq!(p("https://www.deezer.com/es/album/302127?utm_source=x"), Some((Kind::Album, "302127".into())));
        assert_eq!(p("https://deezer.com/playlist/908622995"), Some((Kind::Playlist, "908622995".into())));
        assert_eq!(p("https://www.deezer.com/pt-br/track/1"), Some((Kind::Track, "1".into())));
        assert_eq!(p("https://www.deezer.com/en/artist/27"), None);
        assert_eq!(p("https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"), None);
    }

    #[test]
    fn datos_de_una_cancion() {
        let v: Value = serde_json::from_str(
            r#"{"id":3135556,"title":"Harder, Better","duration":226,"release_date":"2001-03-12","artist":{"name":"Daft Punk"},
            "contributors":[{"name":"Daft Punk"},{"name":"Otro"}],"album":{"title":"Discovery","cover_xl":"https://x/c.jpg"}}"#,
        )
        .unwrap();
        let t = track_of(&v, None, None);
        assert_eq!((t.id.as_str(), t.title.as_str(), t.duration_ms), ("3135556", "Harder, Better", 226_000));
        assert_eq!(t.artists, ["Daft Punk", "Otro"]);
        assert_eq!((t.album.as_deref(), t.year.as_deref(), t.cover_url.as_deref()), (Some("Discovery"), Some("2001"), Some("https://x/c.jpg")));
    }
}
