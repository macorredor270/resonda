// Orquestación de una descarga completa: elegir formatos, descargar y unir.
// Es lo que comparten la CLI y la interfaz gráfica.

use crate::sources::download::download_file;
use crate::sources::youtube::{self, Format, Kind, VideoInfo};
use crate::media::Tags;
use crate::sources::spotify::Track;
use crate::media::flac::FlacWriter;
use crate::media::mp3::{self, Mp3Writer};
use crate::media::wav::WavWriter;
use crate::media::{mp4, pcm};
use crate::sources::matching;
use crate::{Cancelled, Res};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Video,
    Audio,
    Muxing,
}

/// Contenedor del archivo final.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Output {
    /// MP4 (vídeo+audio) o M4A (solo audio), según el plan
    Mp4,
    /// AAC crudo (.aac), solo audio
    Adts,
    /// PCM sin comprimir (.wav), solo audio
    Wav,
    /// Sin pérdida (.flac), solo audio
    Flac,
    /// MP3 (.mp3) a los kbps indicados (0 = 192), solo audio
    Mp3(u32),
}

impl Output {
    /// Extensión del archivo. `audio_only` distingue MP4 (vídeo) de M4A (solo audio en MP4).
    pub fn extension(self, audio_only: bool) -> &'static str {
        match self {
            Output::Mp4 if audio_only => "m4a",
            Output::Mp4 => "mp4",
            Output::Adts => "aac",
            Output::Wav => "wav",
            Output::Flac => "flac",
            Output::Mp3(_) => "mp3",
        }
    }
}

/// Cómo escribir el resultado: contenedor y etiquetas opcionales.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub output: Output,
    pub tags: Option<&'a Tags>,
}

impl Target<'_> {
    pub const MP4: Target<'static> = Target { output: Output::Mp4, tags: None };
}

pub enum Plan {
    /// MP4 ya completo (vídeo+audio) que se guarda tal cual: lo normal en TikTok, y en YouTube
    /// hasta 360p.
    Direct(Format),
    /// MP4 ya completo del que solo se quiere el audio (M4A, AAC, WAV, FLAC o MP3).
    Extract(Format),
    /// Vídeo y audio por separado (o solo audio) que hay que unir con nuestro muxer.
    Merge { video: Option<Format>, audio: Format },
}

impl Plan {
    /// Bytes previstos (vídeo, audio) si se conocen todos; sirve para un progreso global.
    pub fn planned_bytes(&self) -> Option<(u64, u64)> {
        let (v, a) = match self {
            Plan::Direct(f) | Plan::Extract(f) => (f.size, 0),
            Plan::Merge { video, audio } => (video.as_ref().map_or(0, |v| v.size), audio.size),
        };
        let known = match self {
            Plan::Direct(_) | Plan::Extract(_) => v > 0,
            Plan::Merge { video, .. } => a > 0 && video.as_ref().is_none_or(|f| f.size > 0),
        };
        known.then_some((v, a))
    }
}

pub fn plan(info: &VideoInfo, max_height: u32, audio_only: bool) -> Res<Plan> {
    // Servicios que solo dan MP4 completos (TikTok...), y vídeos de YouTube pequeños: sin muxer
    let split = info.formats.iter().any(|f| f.kind == Kind::Video) && info.formats.iter().any(|f| f.kind == Kind::Audio);
    let small = !audio_only && max_height <= 360;
    if !split || small {
        // Con formatos separados solo vale un MP4 completo que respete la altura pedida
        if let Some(f) = youtube::pick_muxed(info, max_height).filter(|f| !split || f.height <= max_height) {
            return Ok(if audio_only { Plan::Extract(f) } else { Plan::Direct(f) });
        }
    }
    let (video, audio) = youtube::pick_formats(info, max_height, audio_only)?;
    Ok(Plan::Merge { video, audio })
}

/// Etiquetas para un archivo de audio sacado de un vídeo: título, autor y miniatura.
pub fn info_tags(agent: &ureq::Agent, info: &VideoInfo) -> Tags {
    // Solo JPEG o PNG: es lo que entienden los reproductores en la portada
    let cover = info.cover_url.as_deref().and_then(|u| fetch_bytes(agent, u)).filter(|b| b.starts_with(&[0xff, 0xd8]) || b.starts_with(b"\x89PNG"));
    Tags { title: info.title.clone(), artist: info.author.clone().unwrap_or_default(), album: None, year: None, cover }
}

pub fn safe_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let name = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    // Los sistemas de archivos limitan el nombre en bytes (255), no en caracteres: con títulos en
    // chino o con emojis 150 caracteres se pasarían. Se deja margen para la extensión y ".video.part".
    let mut name: String = name;
    while name.len() > MAX_NAME_BYTES {
        name.pop();
    }
    // Windows no admite puntos ni espacios al final
    let name = name.trim_end_matches(['.', ' ']).to_string();
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.ends_with(|c: char| c.is_ascii_digit() && c != '0'));
    if name.is_empty() {
        "video".into()
    } else if reserved {
        format!("_{name}")
    } else {
        name
    }
}

/// Largo máximo del nombre (sin extensión). En Windows la ruta entera, con la carpeta y el
/// ".source.part" temporal, no puede pasar de 260 caracteres si no se activaron las rutas largas.
#[cfg(windows)]
const MAX_NAME_BYTES: usize = 100;
#[cfg(not(windows))]
const MAX_NAME_BYTES: usize = 200;

pub fn default_output(info: &VideoInfo, dir: &Path, ext: &str) -> PathBuf {
    dir.join(format!("{}.{ext}", safe_name(&info.title)))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

/// El audio del vídeo ya suelto, si el servicio lo da (el sonido original de un vídeo de TikTok).
pub fn original_sound(info: &VideoInfo) -> Option<&Format> {
    info.formats.iter().find(|f| f.kind == Kind::Audio && f.codec == "sound")
}

#[derive(Debug, PartialEq, Eq)]
enum Container {
    Mp3,
    Mp4,
    Unknown,
}

/// Qué es de verdad un archivo de audio, según sus primeros bytes (las extensiones y los tipos
/// MIME de los servicios no son de fiar).
fn sniff(path: &Path) -> Container {
    use std::io::Read;
    let mut head = [0u8; 12];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    match &head[..n] {
        h if h.starts_with(b"ID3") || (h.len() > 2 && h[0] == 0xff && h[1] & 0xe0 == 0xe0) => Container::Mp3,
        h if h.len() >= 8 && &h[4..8] == b"ftyp" => Container::Mp4,
        _ => Container::Unknown,
    }
}

/// Saca el audio de un plan `Extract`. Si el servicio da el sonido original suelto y sirve para el
/// formato pedido, se usa ese (calidad completa); si no, se baja el MP4 y se saca su audio.
#[allow(clippy::too_many_arguments)]
fn extract_audio(
    agent: &ureq::Agent,
    info: &VideoInfo,
    video: &Format,
    output: Output,
    tags: Option<&Tags>,
    part: &Path,
    tmp: &Path,
    on_progress: &(dyn Fn(Stage, u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Res<()> {
    let progress = |d, t| on_progress(Stage::Audio, d, t);
    // M4A y AAC se quedan con el AAC original del vídeo, tal cual; el resto puede ganar con el sonido
    let wants_decoded = matches!(output, Output::Mp3(_) | Output::Wav | Output::Flac);
    if let Some(snd) = original_sound(info).filter(|_| wants_decoded) {
        match download_file(agent, &snd.url, part, snd.size, &info.headers, &progress, cancel) {
            Err(e) if e.downcast_ref::<Cancelled>().is_some() => return Err(e),
            Ok(()) => {
                on_progress(Stage::Muxing, 0, 0);
                match sniff(part) {
                    Container::Mp3 if matches!(output, Output::Mp3(_)) => return mp3::retag(part, tmp, tags),
                    Container::Mp4 if mp4::AacStream::open(part).is_ok_and(|a| !a.he_aac) => return write_audio(output, part, tmp, tags),
                    _ => {}
                }
            }
            Err(_) => {} // sin el sonido suelto aún queda el audio del vídeo
        }
    }
    download_file(agent, &video.url, part, video.size, &info.headers, &progress, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(Box::new(Cancelled));
    }
    on_progress(Stage::Muxing, 0, 0);
    write_audio(output, part, tmp, tags)
}

/// Convierte el audio AAC de `input` (un MP4 con o sin vídeo) al formato de audio pedido.
fn write_audio(output: Output, input: &Path, dest: &Path, tags: Option<&Tags>) -> Res<()> {
    match output {
        Output::Mp4 => mp4::extract_audio_m4a(input, dest, tags).map(|_| ()),
        Output::Adts => mp4::extract_aac_adts(input, dest).map(|_| ()),
        Output::Wav => pcm::decode_aac_to(input, &mut WavWriter::create(dest, tags)?),
        Output::Flac => pcm::decode_aac_to(input, &mut FlacWriter::create(dest, tags)?),
        Output::Mp3(kbps) => pcm::decode_aac_to(input, &mut Mp3Writer::create(dest, tags, kbps)?),
    }
}

/// Ejecuta el plan y deja el resultado en `out`. `on_progress(etapa, hecho, total)` puede
/// llamarse desde varios hilos. Si `cancel` pasa a true se aborta con `Cancelled` y no queda
/// ningún archivo temporal.
pub fn execute(
    agent: &ureq::Agent,
    info: &VideoInfo,
    plan: &Plan,
    out: &Path,
    target: Target,
    on_progress: &(dyn Fn(Stage, u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Res<()> {
    let Target { output, tags } = target;
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }

    // Todo se escribe en un temporal y solo se renombra al destino si termina bien: un fallo o
    // una cancelación nunca tocan un archivo que ya existiera con ese nombre.
    let tmp = with_suffix(out, ".part");
    let result = match plan {
        Plan::Direct(f) => download_file(agent, &f.url, &tmp, f.size, &info.headers, &|d, t| on_progress(Stage::Video, d, t), cancel),
        Plan::Extract(f) => {
            let part = with_suffix(out, ".source.part");
            let result = extract_audio(agent, info, f, output, tags, &part, &tmp, on_progress, cancel);
            let _ = std::fs::remove_file(&part);
            result
        }
        Plan::Merge { video, audio } => {
            let mut parts: Vec<PathBuf> = Vec::new();
            let result = (|| -> Res<()> {
                if let Some(v) = video {
                    parts.push(with_suffix(out, ".video.part"));
                    let dest = parts.last().unwrap();
                    download_file(agent, &v.url, dest, v.size, &info.headers, &|d, t| on_progress(Stage::Video, d, t), cancel)?;
                }
                parts.push(with_suffix(out, ".audio.part"));
                let dest = parts.last().unwrap();
                download_file(agent, &audio.url, dest, audio.size, &info.headers, &|d, t| on_progress(Stage::Audio, d, t), cancel)?;

                if cancel.load(Ordering::Relaxed) {
                    return Err(Box::new(Cancelled));
                }
                on_progress(Stage::Muxing, 0, 0);
                let audio = parts.last().unwrap();
                match output {
                    Output::Mp4 => mp4::mux_mp4(&parts, &tmp, tags, |_, _| {}).map(|_| ()),
                    _ => write_audio(output, audio, &tmp, tags),
                }
            })();
            for p in &parts {
                let _ = std::fs::remove_file(p);
            }
            result
        }
    };
    match result {
        Ok(()) => Ok(std::fs::rename(&tmp, out)?),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

// ---------- canciones de Spotify (metadatos de Spotify, audio de YouTube) ----------

/// Qué vídeo de YouTube se eligió para una canción y con qué confianza.
#[derive(Debug, Clone)]
pub struct Matched {
    pub video_id: String,
    pub title: String,
    pub score: f64,
}

/// Busca la canción en YouTube y devuelve los mejores candidatos (título, artista y duración),
/// del mejor al peor. Si la primera consulta no da una coincidencia muy clara, se prueba otra y
/// se juntan los resultados.
pub fn find_on_youtube(agent: &ureq::Agent, track: &Track) -> Res<Vec<Matched>> {
    let artists = track.artists.join(" ");
    let queries = [format!("{artists} - {}", track.title), format!("{} {} topic", track.main_artist(), track.title)];
    let mut pool: Vec<youtube::SearchResult> = Vec::new();
    let mut ranked = Vec::new();
    for q in &queries {
        for r in youtube::search(agent, q)? {
            if !pool.iter().any(|p| p.id == r.id) {
                pool.push(r);
            }
        }
        ranked = matching::ranked(track, &pool);
        if ranked.first().is_some_and(|b| b.score >= 100.0) {
            break;
        }
    }
    if ranked.is_empty() {
        return Err(format!("No encontré «{}» en YouTube con un título y una duración parecidos", track.label()).into());
    }
    Ok(ranked.into_iter().take(4).map(|b| Matched { video_id: b.result.id, title: b.result.title, score: b.score.min(100.0) }).collect())
}

fn fetch_bytes(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let resp = agent.get(url).call().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let mut buf = Vec::new();
    resp.into_body().into_reader().take(10 * 1024 * 1024).read_to_end(&mut buf).ok()?;
    (!buf.is_empty()).then_some(buf)
}

/// Descarga una canción de Spotify en el formato de audio pedido (M4A, AAC, WAV, FLAC o MP3), con
/// sus etiquetas y su portada.
/// Si el mejor candidato no se puede bajar completo (YouTube corta algunos vídeos), prueba el
/// siguiente. Devuelve la ruta del archivo y el vídeo de YouTube que se usó.
pub fn download_spotify_track(
    agent: &ureq::Agent,
    track: &Track,
    dir: &Path,
    output: Option<&Path>,
    format: Output,
    on_progress: &(dyn Fn(Stage, u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Res<(PathBuf, Matched)> {
    let candidates = find_on_youtube(agent, track)?;
    let tags = Tags {
        title: track.title.clone(),
        artist: track.artists.join(", "),
        album: track.album.clone(),
        year: track.year.clone(),
        cover: track.cover_url.as_deref().and_then(|u| fetch_bytes(agent, u)),
    };
    let ext = format.extension(true);
    let out = output.map(Path::to_path_buf).unwrap_or_else(|| dir.join(format!("{}.{ext}", safe_name(&track.label()))));

    let mut last_error: Box<dyn std::error::Error + Send + Sync> = "Sin candidatos".into();
    for matched in candidates {
        if cancel.load(Ordering::Relaxed) {
            return Err(Box::new(Cancelled));
        }
        let attempt = || -> Res<()> {
            let info = youtube::get_video_info(agent, &matched.video_id)?;
            let plan = plan(&info, 0, true)?;
            execute(agent, &info, &plan, &out, Target { output: format, tags: Some(&tags) }, on_progress, cancel)
        };
        match attempt() {
            Ok(()) => return Ok((out, matched)),
            Err(e) if e.downcast_ref::<Cancelled>().is_some() => return Err(e),
            Err(e) => last_error = e, // se prueba con el siguiente candidato
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nombre_seguro_quita_caracteres_prohibidos() {
        assert_eq!(safe_name("AC/DC: Back  in <Black>?"), "AC_DC_ Back in _Black__");
        assert_eq!(safe_name("   "), "video");
    }

    #[test]
    fn nombre_seguro_limita_bytes_no_caracteres() {
        let name = safe_name(&"曲".repeat(150));
        assert!(name.len() <= MAX_NAME_BYTES);
        assert!(name.chars().all(|c| c == '曲'));
    }

    #[test]
    fn nombre_seguro_evita_nombres_de_windows() {
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("com1"), "_com1");
        assert_eq!(safe_name("Fin..."), "Fin");
        assert_eq!(safe_name("COM0"), "COM0");
    }
}

#[cfg(test)]
mod sniff_tests {
    use super::*;

    #[test]
    fn reconoce_el_contenedor_por_los_bytes() {
        let dir = std::env::temp_dir().join(format!("resonda-sniff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let check = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            sniff(&p)
        };
        assert_eq!(check("a.mp3", b"ID3\x04\0\0\0\0\0\0"), Container::Mp3);
        assert_eq!(check("b.mp3", &[0xff, 0xfb, 0x90, 0x00, 0, 0, 0, 0]), Container::Mp3);
        assert_eq!(check("c.m4a", b"\0\0\0\x18ftypM4A \0\0"), Container::Mp4);
        assert_eq!(check("d.bin", b"nada"), Container::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
