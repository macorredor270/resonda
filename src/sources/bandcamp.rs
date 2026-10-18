// Bandcamp: la página de cada canción lleva un JSON (`data-tralbum`) con la URL de un MP3 de
// 128 kbps para escuchar sin cuenta. Es solo audio y siempre MP3.
//
// Si Bandcamp cambia algo, lo normal es que solo haya que tocar `info_from_html`.

use crate::sources::youtube::{Format, Kind, VideoInfo};
use crate::sources::{html_unescape, on_domain, retry_call, Extractor};
use crate::Res;
use serde_json::Value;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub struct Bandcamp;

impl Extractor for Bandcamp {
    fn id(&self) -> &'static str {
        "bandcamp"
    }
    fn name(&self) -> &'static str {
        "Bandcamp"
    }
    fn matches(&self, input: &str) -> bool {
        on_domain(input, "bandcamp.com")
    }
    fn extract(&self, agent: &ureq::Agent, input: &str) -> Res<VideoInfo> {
        let url = if input.contains("://") { input.to_string() } else { format!("https://{input}") };
        let mut resp = retry_call(|| agent.get(&url).header("User-Agent", UA).call())?;
        if !resp.status().is_success() {
            return Err(format!("Bandcamp respondió HTTP {}", resp.status().as_u16()).into());
        }
        let html = resp.body_mut().read_to_string()?;
        info_from_html(&html)
    }
}

/// Valor de un atributo HTML (`name="..."`), ya sin escapes.
fn attr(html: &str, name: &str) -> Option<String> {
    let start = html.find(&format!("{name}=\""))? + name.len() + 2;
    Some(html_unescape(&html[start..start + html[start..].find('"')?]))
}

pub(crate) fn info_from_html(html: &str) -> Res<VideoInfo> {
    let json = attr(html, "data-tralbum").ok_or("Bandcamp no entregó los datos de la canción (¿no es una página de canción o álbum?)")?;
    let data: Value = serde_json::from_str(&json).map_err(|e| format!("Datos de Bandcamp ilegibles: {e}"))?;
    let tracks = data["trackinfo"].as_array().map(Vec::as_slice).unwrap_or_default();
    let track = match tracks {
        [one] => one,
        [] => return Err("Esta página de Bandcamp no tiene canciones".into()),
        many => return Err(format!("Esta página de Bandcamp es un álbum de {} canciones: pega el enlace de una canción", many.len()).into()),
    };
    let url = track["file"]["mp3-128"].as_str().ok_or("Esta canción de Bandcamp no se puede escuchar sin comprarla")?;

    let title = track["title"].as_str().unwrap_or("Bandcamp").to_string();
    let cover_url = html
        .split_once("property=\"og:image\" content=\"")
        .and_then(|(_, r)| r.split('"').next())
        .filter(|u| u.starts_with("http"))
        .map(html_unescape);
    Ok(VideoInfo {
        service: "bandcamp",
        id: data["id"].as_u64().map(|n| n.to_string()).unwrap_or_default(),
        title,
        author: data["artist"].as_str().map(str::to_string),
        seconds: track["duration"].as_f64().unwrap_or(0.0).round() as u64,
        formats: vec![Format { itag: 0, url: url.to_string(), kind: Kind::Audio, codec: "sound".into(), height: 0, fps: 0, bitrate: 128_000, size: 0 }],
        cover_url,
        headers: vec![("User-Agent".into(), UA.into())],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(tracks: &str) -> String {
        let json = format!(r#"{{"id":42,"artist":"Los \"Tests\"","trackinfo":{tracks}}}"#).replace('"', "&quot;");
        format!(r#"<meta property="og:image" content="https://f4.bcbits.com/img/a1.jpg"><div data-tralbum="{json}"></div>"#)
    }

    #[test]
    fn lee_una_cancion() {
        let i = info_from_html(&page(r#"[{"title":"Hola &amp; adiós","duration":116.37,"file":{"mp3-128":"https://t4.bcbits.com/s.mp3?a=1&b=2"}}]"#.replace("&amp;", "&").as_str())).unwrap();
        assert_eq!((i.service, i.id.as_str(), i.title.as_str(), i.seconds), ("bandcamp", "42", "Hola & adiós", 116));
        assert_eq!(i.author.as_deref(), Some("Los \"Tests\""));
        assert_eq!(i.cover_url.as_deref(), Some("https://f4.bcbits.com/img/a1.jpg"));
        assert_eq!(i.formats.len(), 1);
        assert_eq!(i.formats[0].url, "https://t4.bcbits.com/s.mp3?a=1&b=2");
        assert!(crate::pipeline::original_sound(&i).is_some());
    }

    #[test]
    fn paginas_danadas_no_hacen_panico() {
        let html = page(r#"[{"title":"Hola","duration":116.37,"file":{"mp3-128":"https://t4.bcbits.com/s.mp3?a=1&b=2"}}]"#);
        let mut rng = crate::testutil::Rng(12);
        for h in crate::testutil::mutations(&html, &mut rng, 3000) {
            let _ = info_from_html(&h);
        }
        for weird in ["null", "[]", r#"{"trackinfo":"x"}"#, r#"{"trackinfo":[null]}"#, r#"{"trackinfo":[{"file":7,"duration":"x"}],"id":"no"}"#, r#"{"trackinfo":[{"title":5,"file":{"mp3-128":1}}]}"#] {
            let _ = info_from_html(&format!("<div data-tralbum=\"{}\"></div>", weird.replace('"', "&quot;")));
        }
    }

    #[test]
    fn errores_claros() {
        let album = page(r#"[{"title":"a","file":{"mp3-128":"u"}},{"title":"b","file":{"mp3-128":"u"}}]"#);
        assert!(info_from_html(&album).unwrap_err().to_string().contains("álbum de 2"));
        let paid = page(r#"[{"title":"a","file":null}]"#);
        assert!(info_from_html(&paid).unwrap_err().to_string().contains("sin comprarla"));
        assert!(info_from_html("<html></html>").is_err());
    }
}
