// TikTok: la página pública de cada vídeo lleva sus datos en un JSON incrustado
// (`__UNIVERSAL_DATA_FOR_REHYDRATION__`), con las URLs de varios MP4 ya completos (vídeo + audio,
// sin marca de agua). Hay que abrir antes la página y reutilizar las cookies que reparte a
// cualquier visitante: sin ellas el servidor de vídeo responde 403.
//
// Si TikTok cambia algo, lo normal es que solo haya que tocar `info_from_html`.

use crate::sources::youtube::{Format, Kind, VideoInfo};
use crate::sources::{cookie_header, on_domain, Extractor};
use crate::Res;
use serde_json::Value;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const REFERER: &str = "https://www.tiktok.com/";

pub struct TikTok;

impl Extractor for TikTok {
    fn id(&self) -> &'static str {
        "tiktok"
    }
    fn name(&self) -> &'static str {
        "TikTok"
    }
    fn matches(&self, input: &str) -> bool {
        on_domain(input, "tiktok.com")
    }
    fn extract(&self, agent: &ureq::Agent, input: &str) -> Res<VideoInfo> {
        let url = if input.contains("://") { input.to_string() } else { format!("https://{input}") };
        let url = resolve_short_link(&url)?;
        let mut resp = agent
            .get(&url)
            .header("User-Agent", UA)
            .header("Accept-Language", "en-us,en;q=0.5")
            .call()?;
        if !resp.status().is_success() {
            return Err(format!("TikTok respondió HTTP {}", resp.status().as_u16()).into());
        }
        let cookies = cookie_header(resp.headers());
        let html = resp.body_mut().read_to_string()?;
        info_from_html(&html, &cookies)
    }
}

/// Los enlaces cortos (`vm.tiktok.com/XXX`, `vt.tiktok.com/XXX`, `tiktok.com/t/XXX`) redirigen
/// a la página del vídeo. Se siguen a mano, sin descargar nada, hasta llegar a `/video/<id>`.
fn resolve_short_link(url: &str) -> Res<String> {
    let host = crate::sources::host_of(url).unwrap_or_default();
    let is_short = matches!(host.as_str(), "vm.tiktok.com" | "vt.tiktok.com") || url.contains("tiktok.com/t/");
    if !is_short {
        return Ok(url.to_string());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut current = url.to_string();
    for _ in 0..5 {
        if current.contains("/video/") || current.contains("/photo/") {
            return Ok(current);
        }
        let resp = agent.get(&current).header("User-Agent", UA).call()?;
        let next = resp.headers().get("location").and_then(|v| v.to_str().ok()).ok_or("El enlace corto de TikTok no redirige a ningún vídeo")?;
        current = if next.starts_with("http") { next.to_string() } else { format!("https://www.tiktok.com{next}") };
    }
    Ok(current)
}

/// Saca el JSON que TikTok incrusta en la página y lo convierte en `VideoInfo`.
/// `cookies` va a las cabeceras de descarga.
pub(crate) fn info_from_html(html: &str, cookies: &str) -> Res<VideoInfo> {
    let marker = "id=\"__UNIVERSAL_DATA_FOR_REHYDRATION__\"";
    let after = &html[html.find(marker).ok_or("TikTok no entregó los datos del vídeo (¿pide verificación o cambió la página?)")? + marker.len()..];
    let json = after.split_once('>').and_then(|(_, r)| r.split_once("</script>")).map(|(j, _)| j).ok_or("Datos de TikTok incompletos")?;
    let data: Value = serde_json::from_str(json).map_err(|e| format!("Datos de TikTok ilegibles: {e}"))?;
    let detail = &data["__DEFAULT_SCOPE__"]["webapp.video-detail"];

    let item = &detail["itemInfo"]["itemStruct"];
    if !item.is_object() {
        let msg = match detail["statusCode"].as_u64() {
            Some(10204) => "el vídeo no existe o fue eliminado".to_string(),
            Some(10222) => "el vídeo es privado".to_string(),
            Some(10216) => "el vídeo no está disponible en tu país".to_string(),
            _ => detail["statusMsg"].as_str().filter(|s| !s.is_empty()).unwrap_or("vídeo no disponible").to_string(),
        };
        return Err(format!("TikTok: {msg}").into());
    }
    if item["imagePost"].is_object() {
        return Err("Esta publicación de TikTok es un carrusel de fotos, no un vídeo".into());
    }

    let id = item["id"].as_str().unwrap_or_default().to_string();
    let video = &item["video"];
    let mut formats = Vec::new();
    for b in video["bitrateInfo"].as_array().into_iter().flatten() {
        let addr = &b["PlayAddr"];
        let Some(url) = addr["UrlList"].as_array().and_then(|l| l.first()).and_then(Value::as_str) else { continue };
        let (w, h) = (addr["Width"].as_u64().unwrap_or(0), addr["Height"].as_u64().unwrap_or(0));
        let codec = match b["CodecType"].as_str().unwrap_or("") {
            c if c.starts_with("h264") => "avc1",
            c if c.starts_with("h265") => "hvc1",
            c if c.contains("av1") => "av01",
            _ => continue,
        };
        formats.push(Format {
            itag: b["QualityType"].as_u64().unwrap_or(0) as u32,
            url: url.to_string(),
            kind: Kind::Muxed,
            codec: codec.to_string(),
            // Como en YouTube, la calidad es el lado corto: un vídeo vertical de 1080x1920 es 1080p
            height: w.min(h) as u32,
            fps: 0,
            bitrate: b["Bitrate"].as_u64().unwrap_or(0),
            size: addr["DataSize"].as_str().and_then(|s| s.parse().ok()).or(addr["DataSize"].as_u64()).unwrap_or(0),
        });
    }
    // Sin lista de calidades, el vídeo "normal" de la página
    if formats.is_empty() {
        if let Some(url) = video["playAddr"].as_str().filter(|u| !u.is_empty()) {
            let (w, h) = (video["width"].as_u64().unwrap_or(0), video["height"].as_u64().unwrap_or(0));
            formats.push(Format {
                itag: 0,
                url: url.to_string(),
                kind: Kind::Muxed,
                codec: "avc1".into(),
                height: w.min(h) as u32,
                fps: 0,
                bitrate: video["bitrate"].as_u64().unwrap_or(0),
                size: video["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
            });
        }
    }
    if formats.is_empty() {
        return Err("TikTok no ofrece ningún formato descargable para este vídeo".into());
    }

    // El sonido original de un vídeo es el mismo audio del vídeo, pero suelto y de calidad completa
    // (MP3 de 128 kbps o AAC-LC), mientras que el audio del MP4 es HE-AAC, que Resonda no sabe
    // decodificar entero. Si el vídeo lleva música de otro (no es original), esa pista no es el
    // audio del vídeo. Cuál de los dos contenedores es solo se sabe al bajarlo.
    if item["music"]["original"].as_bool() == Some(true) {
        if let Some(url) = item["music"]["playUrl"].as_str().filter(|u| u.starts_with("http")) {
            formats.push(Format { itag: 0, url: url.to_string(), kind: Kind::Audio, codec: "sound".into(), height: 0, fps: 0, bitrate: 128_000, size: 0 });
        }
    }

    let author = item["author"]["uniqueId"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let desc = item["desc"].as_str().unwrap_or("").trim();
    let title = if desc.is_empty() {
        format!("{} - {id}", author.as_deref().unwrap_or("TikTok"))
    } else {
        desc.to_string()
    };
    let seconds = video["duration"].as_u64().or_else(|| video["duration"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
    let cover_url = ["originCover", "cover"].iter().find_map(|k| video[*k].as_str().filter(|u| !u.is_empty())).map(str::to_string);

    let mut headers = vec![("User-Agent".to_string(), UA.to_string()), ("Referer".to_string(), REFERER.to_string())];
    if !cookies.is_empty() {
        headers.push(("Cookie".to_string(), cookies.to_string()));
    }
    Ok(VideoInfo { service: "tiktok", id, title, author, seconds, formats, cover_url, headers })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(inner: &str) -> String {
        format!(r#"<html><script id="__UNIVERSAL_DATA_FOR_REHYDRATION__" type="application/json">{inner}</script></html>"#)
    }

    #[test]
    fn lee_formatos_titulo_y_cabeceras() {
        let html = page(
            r#"{"__DEFAULT_SCOPE__":{"webapp.video-detail":{"statusCode":0,"itemInfo":{"itemStruct":{
            "id":"123","desc":"  Hola #mundo ","author":{"uniqueId":"ana"},
            "music":{"original":true,"playUrl":"https://x/sound.mp3"},"video":{"duration":24,"cover":"https://x/c.jpg","bitrateInfo":[
              {"QualityType":2,"Bitrate":1000,"CodecType":"h265_hvc1","PlayAddr":{"UrlList":["https://x/h265"],"DataSize":"300","Width":1080,"Height":1920}},
              {"QualityType":20,"Bitrate":900,"CodecType":"h264","PlayAddr":{"UrlList":["https://x/h264"],"DataSize":"400","Width":576,"Height":1024}}]}}}}}}"#,
        );
        let i = info_from_html(&html, "a=1; b=2").unwrap();
        assert_eq!((i.service.to_string(), i.id.as_str(), i.title.as_str(), i.seconds), ("tiktok".into(), "123", "Hola #mundo", 24));
        assert_eq!(i.author.as_deref(), Some("ana"));
        assert_eq!(i.cover_url.as_deref(), Some("https://x/c.jpg"));
        let f: Vec<_> = i.formats.iter().map(|f| (f.codec.as_str(), f.height, f.size)).collect();
        assert_eq!(f, [("hvc1", 1080, 300), ("avc1", 576, 400), ("sound", 0, 0)]);
        assert_eq!(crate::pipeline::original_sound(&i).map(|f| f.url.as_str()), Some("https://x/sound.mp3"));
        assert!(i.headers.contains(&("Cookie".into(), "a=1; b=2".into())));
        assert!(i.headers.iter().any(|(k, v)| k == "Referer" && v.contains("tiktok.com")));
    }

    #[test]
    fn errores_claros() {
        let missing = page(r#"{"__DEFAULT_SCOPE__":{"webapp.video-detail":{"statusCode":10204,"statusMsg":""}}}"#);
        assert!(info_from_html(&missing, "").unwrap_err().to_string().contains("no existe"));
        let photos = page(r#"{"__DEFAULT_SCOPE__":{"webapp.video-detail":{"itemInfo":{"itemStruct":{"id":"1","imagePost":{},"video":{}}}}}}"#);
        assert!(info_from_html(&photos, "").unwrap_err().to_string().contains("fotos"));
        assert!(info_from_html("<html></html>", "").is_err());
    }
}
