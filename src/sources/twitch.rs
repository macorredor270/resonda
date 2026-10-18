// Twitch (clips): la web de Twitch pide los clips a su API GraphQL con un identificador de cliente
// público (el mismo para todo el mundo, va dentro de la propia web). La respuesta trae las URLs de
// MP4 normales (H.264 + AAC) y un token de acceso corto que se añade a cada URL. Solo clips: los
// vídeos bajo demanda y los directos van por HLS y no están soportados.
//
// Si Twitch cambia algo, lo normal es que solo haya que tocar `QUERY` o `info_from_json`.

use crate::sources::youtube::{Format, Kind, VideoInfo};
use crate::sources::{host_of, on_domain, percent_encode, retry_call, Extractor};
use crate::Res;
use serde_json::{json, Value};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
const QUERY: &str = r#"query($slug: ID!) { clip(slug: $slug) {
    id title durationSeconds broadcaster { displayName } thumbnailURL(width: 480, height: 272)
    playbackAccessToken(params: {platform: "web", playerBackend: "mediaplayer", playerType: "site"}) { signature value }
    videoQualities { frameRate quality sourceURL }
} }"#;

pub struct Twitch;

impl Extractor for Twitch {
    fn id(&self) -> &'static str {
        "twitch"
    }
    fn name(&self) -> &'static str {
        "Twitch"
    }
    fn matches(&self, input: &str) -> bool {
        on_domain(input, "twitch.tv")
    }
    fn extract(&self, agent: &ureq::Agent, input: &str) -> Res<VideoInfo> {
        let slug = clip_slug(input).ok_or("Twitch: solo se pueden descargar clips (clips.twitch.tv/… o twitch.tv/canal/clip/…)")?;
        let body = json!({ "query": QUERY, "variables": { "slug": slug } });
        let mut resp = retry_call(|| agent.post("https://gql.twitch.tv/gql").header("Client-ID", CLIENT_ID).header("User-Agent", UA).send_json(&body))?;
        if !resp.status().is_success() {
            return Err(format!("Twitch respondió HTTP {}", resp.status().as_u16()).into());
        }
        info_from_json(&resp.body_mut().read_json::<Value>()?)
    }
}

fn is_slug(s: &str) -> bool {
    !s.is_empty() && s.len() <= 120 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// El identificador (slug) de un clip: `clips.twitch.tv/<slug>`, `clips.twitch.tv/embed?clip=<slug>`,
/// `twitch.tv/<canal>/clip/<slug>` o `m.twitch.tv/clip/<slug>`.
pub(crate) fn clip_slug(input: &str) -> Option<String> {
    let host = host_of(input)?;
    let rest = input.split_once("://").map_or(input, |(_, r)| r);
    let (path, query) = rest.split_once('/').map_or(("", ""), |(_, p)| p.split_once('?').unwrap_or((p, "")));
    let path = path.split('#').next().unwrap_or("");
    let query = query.split('#').next().unwrap_or("");
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let slug = if host == "clips.twitch.tv" {
        match segs.first() {
            Some(&"embed") => query.split('&').find_map(|kv| kv.strip_prefix("clip=")),
            other => other.copied(),
        }
    } else {
        segs.iter().position(|s| *s == "clip").and_then(|i| segs.get(i + 1)).copied()
    }?;
    is_slug(slug).then(|| slug.to_string())
}

pub(crate) fn info_from_json(resp: &Value) -> Res<VideoInfo> {
    if let Some(msg) = resp["errors"][0]["message"].as_str() {
        return Err(format!("Twitch: {msg}").into());
    }
    let clip = &resp["data"]["clip"];
    if !clip.is_object() {
        return Err("Twitch: el clip no existe o fue eliminado".into());
    }
    let (sig, token) = (clip["playbackAccessToken"]["signature"].as_str(), clip["playbackAccessToken"]["value"].as_str());
    let (Some(sig), Some(token)) = (sig, token) else { return Err("Twitch no dio acceso a este clip".into()) };

    let mut formats = Vec::new();
    for q in clip["videoQualities"].as_array().into_iter().flatten() {
        let Some(url) = q["sourceURL"].as_str() else { continue };
        formats.push(Format {
            itag: 0,
            url: format!("{url}?sig={sig}&token={}", percent_encode(token)),
            kind: Kind::Muxed,
            codec: "avc1".into(),
            height: q["quality"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
            fps: q["frameRate"].as_f64().unwrap_or(0.0).round() as u32,
            bitrate: 0,
            size: 0,
        });
    }
    if formats.is_empty() {
        return Err("Twitch no ofrece ningún formato descargable para este clip".into());
    }
    Ok(VideoInfo {
        service: "twitch",
        id: clip["id"].as_str().unwrap_or_default().to_string(),
        title: clip["title"].as_str().unwrap_or("Clip de Twitch").to_string(),
        author: clip["broadcaster"]["displayName"].as_str().map(str::to_string),
        seconds: clip["durationSeconds"].as_f64().unwrap_or(0.0).round() as u64,
        formats,
        cover_url: clip["thumbnailURL"].as_str().filter(|u| u.starts_with("http")).map(str::to_string),
        headers: vec![("User-Agent".into(), UA.into())],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enlaces_de_clips() {
        let slug = "BrainyVictoriousPeppermintCurseLit-OcD0FV0YN5Nm4Gk1";
        for input in [
            format!("https://clips.twitch.tv/{slug}"),
            format!("https://clips.twitch.tv/{slug}?tt_medium=x"),
            format!("clips.twitch.tv/{slug}"),
            format!("https://www.twitch.tv/thechief1114/clip/{slug}?filter=clips&range=all"),
            format!("https://m.twitch.tv/clip/{slug}"),
            format!("https://clips.twitch.tv/embed?clip={slug}&parent=example.com"),
        ] {
            assert_eq!(clip_slug(&input).as_deref(), Some(slug), "{input}");
        }
        assert_eq!(clip_slug("https://www.twitch.tv/videos/123456"), None);
        assert_eq!(clip_slug("https://www.twitch.tv/xqc"), None);
    }

    #[test]
    fn lee_calidades_y_firma_la_url() {
        let r = json!({"data":{"clip":{"id":"1","title":"Gran jugada","durationSeconds":33.4,"broadcaster":{"displayName":"Canal"},
            "thumbnailURL":"https://x/t.jpg","playbackAccessToken":{"signature":"abc","value":"{\"a\":1}"},
            "videoQualities":[{"quality":"1080","frameRate":60.0,"sourceURL":"https://c/1080.mp4"},{"quality":"360","frameRate":29.97,"sourceURL":"https://c/360.mp4"}]}}});
        let i = info_from_json(&r).unwrap();
        assert_eq!((i.service, i.title.as_str(), i.seconds, i.author.as_deref()), ("twitch", "Gran jugada", 33, Some("Canal")));
        let f: Vec<_> = i.formats.iter().map(|f| (f.height, f.fps)).collect();
        assert_eq!(f, [(1080, 60), (360, 30)]);
        assert_eq!(i.formats[0].url, "https://c/1080.mp4?sig=abc&token=%7B%22a%22%3A1%7D");
        let p = crate::sources::youtube::pick_muxed(&i, 720).unwrap();
        assert_eq!(p.height, 360);
    }

    #[test]
    fn respuestas_danadas_no_hacen_panico() {
        let json = r#"{"data":{"clip":{"id":"1","title":"Gran jugada","durationSeconds":33.4,"broadcaster":{"displayName":"Canal"},
            "thumbnailURL":"https://x/t.jpg","playbackAccessToken":{"signature":"abc","value":"{\"a\":1}"},
            "videoQualities":[{"quality":"1080","frameRate":60.0,"sourceURL":"https://c/1080.mp4"}]}}}"#;
        let mut rng = crate::testutil::Rng(13);
        for text in crate::testutil::mutations(json, &mut rng, 3000) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let _ = info_from_json(&v);
            }
        }
        for weird in [json!(null), json!([]), json!({"data":7}), json!({"data":{"clip":{"playbackAccessToken":{"signature":1,"value":2},"videoQualities":{}}}}), json!({"errors":[]}), json!({"errors":[{"message":5}]})] {
            let _ = info_from_json(&weird);
        }
    }

    #[test]
    fn errores_claros() {
        assert!(info_from_json(&json!({"data":{"clip":null}})).unwrap_err().to_string().contains("no existe"));
        assert!(info_from_json(&json!({"errors":[{"message":"boom"}]})).unwrap_err().to_string().contains("boom"));
    }
}
