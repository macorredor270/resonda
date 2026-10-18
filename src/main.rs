// Resonda: descarga vídeos de YouTube como MP4, sin yt-dlp ni ffmpeg.
// Habla con la API interna de YouTube (sources/youtube.rs), descarga por trozos
// (sources/download.rs) y une vídeo+audio con un muxer MP4 propio (media/mp4/).

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use resonda::pipeline::{self, Output, Plan, Stage, Target};
use resonda::sources::spotify::Kind as SpotKind;
use resonda::sources::youtube::{Kind, VideoInfo};
use resonda::sources::{self, Catalog, CATALOGS, EXTRACTORS};
use resonda::{new_agent, Res};

const HELP: &str = "Uso: resonda [opciones] <URL o ID> [<URL o ID> ...]

Acepta enlaces de YouTube, TikTok, Bandcamp y clips de Twitch, y de Spotify, Deezer, Apple Music y
Tidal; -S los lista. De estos cuatro últimos (canción, álbum o playlist) solo se leen los datos
(título, artista, portada): el audio se busca y se descarga desde YouTube como .m4a con sus etiquetas.

Opciones:
  -q, --quality <alto>   Altura máxima en píxeles, o \"best\" (por defecto 1080).
                         Por encima de 1080 YouTube solo ofrece AV1.
  -o, --output <ruta>    Archivo de salida (solo con un vídeo). Por defecto \"<título>.mp4\"
  -d, --dir <carpeta>    Carpeta de salida (por defecto la actual)
  -f, --format <fmt>     mp4 (vídeo, por defecto) | mp3 | flac | wav | m4a | aac
  -b, --bitrate <kbps>   Calidad del MP3: 128, 192 (por defecto), 256 o 320
  -a, --audio            Atajo de --format m4a
  -n, --limit <N>        Con álbumes/playlists: descargar solo las N primeras canciones
  -l, --list             Mostrar los formatos disponibles (o las canciones) y salir
  -S, --services         Mostrar los servicios soportados y salir
  -V, --version          Mostrar la versión
  -h, --help             Esta ayuda";

struct Options {
    quality: String,
    output: Option<PathBuf>,
    dir: PathBuf,
    format: String,
    bitrate: u32,
    list: bool,
    limit: Option<usize>,
    inputs: Vec<String>,
}

fn parse_args() -> Res<Option<Options>> {
    let mut o = Options { quality: "1080".into(), output: None, dir: ".".into(), format: "mp4".into(), bitrate: 0, list: false, limit: None, inputs: vec![] };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        // Admite "--opcion=valor"
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Res<String> {
            inline.clone().or_else(|| args.next()).ok_or_else(|| format!("Falta el valor de {name}").into())
        };
        match flag.as_str() {
            "-q" | "--quality" => o.quality = value("--quality")?,
            "-o" | "--output" => o.output = Some(value("--output")?.into()),
            "-d" | "--dir" => o.dir = value("--dir")?.into(),
            "-a" | "--audio" => o.format = "m4a".into(),
            "-f" | "--format" => o.format = value("--format")?.to_lowercase(),
            "-b" | "--bitrate" => o.bitrate = value("--bitrate")?.parse().map_err(|_| "El valor de --bitrate debe ser un número")?,
            "-l" | "--list" => o.list = true,
            "-n" | "--limit" => {
                o.limit = Some(value("--limit")?.parse().map_err(|_| "El valor de --limit debe ser un número")?)
            }
            "-S" | "--services" => {
                for e in EXTRACTORS {
                    println!("{:<10} {}", e.id(), e.name());
                }
                for c in CATALOGS {
                    println!("{:<10} {} (solo metadatos; el audio sale de YouTube)", c.id(), c.name());
                }
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("resonda {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-h" | "--help" => return Ok(None),
            f if f.starts_with('-') && f.len() > 1 => return Err(format!("Opción desconocida: {f}").into()),
            _ => o.inputs.push(arg),
        }
    }
    if o.bitrate != 0 && ![128, 192, 256, 320].contains(&o.bitrate) {
        return Err("El valor de --bitrate debe ser 128, 192, 256 o 320".into());
    }
    Ok(Some(o))
}

fn mb(n: u64) -> String {
    format!("{:.1}", n as f64 / 1_048_576.0)
}

struct Progress {
    label: &'static str,
    started: Instant,
    last_draw: Mutex<Instant>,
    tty: bool,
}

impl Progress {
    fn new(label: &'static str) -> Self {
        Progress { label, started: Instant::now(), last_draw: Mutex::new(Instant::now() - Duration::from_secs(1)), tty: std::io::stderr().is_terminal() }
    }

    fn update(&self, done: u64, total: u64) {
        if !self.tty {
            return;
        }
        let now = Instant::now();
        {
            let mut last = self.last_draw.lock().unwrap();
            if done < total && now.duration_since(*last) < Duration::from_millis(100) {
                return;
            }
            *last = now;
        }
        let speed = done as f64 / self.started.elapsed().as_secs_f64().max(0.001);
        eprint!(
            "\r  {:<7} {:>5.1}%  {}/{} MB  {:.1} MB/s   ",
            self.label,
            done as f64 / total as f64 * 100.0,
            mb(done),
            mb(total),
            speed / 1_048_576.0
        );
        if done >= total {
            eprintln!();
        }
    }
}

fn print_formats(info: &VideoInfo) {
    println!("  itag  tipo    códec  altura  fps  tamaño");
    for f in &info.formats {
        let kind = match f.kind {
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Muxed => "muxed",
        };
        let height = if f.height > 0 { format!("{}p", f.height) } else { "-".into() };
        let fps = if f.fps > 0 { f.fps.to_string() } else { "-".into() };
        let size = if f.size > 0 { format!("{} MB", mb(f.size)) } else { "-".into() };
        println!("  {:<5} {kind:<7} {:<6} {height:<7} {fps:<4} {size}", f.itag, f.codec);
    }
}

/// Formato pedido en la línea de comandos. Devuelve el contenedor y si es solo audio.
fn output_of(opts: &Options) -> Res<(Output, bool)> {
    let output = match opts.format.as_str() {
        "mp4" | "m4a" => Output::Mp4,
        "aac" => Output::Adts,
        "wav" => Output::Wav,
        "flac" => Output::Flac,
        "mp3" => Output::Mp3(opts.bitrate),
        other => return Err(format!("Formato no soportado: \"{other}\" (usa mp4, mp3, flac, wav, m4a o aac)").into()),
    };
    Ok((output, opts.format != "mp4"))
}

fn fetch_one(agent: &ureq::Agent, opts: &Options, input: &str) -> Res<()> {
    let info = sources::fetch_info(agent, input)?;
    println!("{}  [{}: {}]  {}:{:02}", info.title, info.service, info.id, info.seconds / 60, info.seconds % 60);

    if opts.list {
        print_formats(&info);
        return Ok(());
    }

    // Servicios de solo audio (Bandcamp): sin elegir formato, MP3 en vez de un MP4 que no existe
    let audio_service = !info.formats.iter().any(|f| matches!(f.kind, Kind::Video | Kind::Muxed));
    let adjusted;
    let opts = if audio_service && opts.format == "mp4" {
        adjusted = Options { format: "mp3".into(), quality: opts.quality.clone(), output: opts.output.clone(), dir: opts.dir.clone(), inputs: vec![], ..*opts };
        &adjusted
    } else {
        opts
    };

    let max_height: u32 = if opts.quality == "best" {
        u32::MAX
    } else {
        opts.quality.parse().ok().filter(|&h| h > 0).ok_or_else(|| format!("Calidad no válida: \"{}\"", opts.quality))?
    };

    let (output, audio_only) = output_of(opts)?;
    let plan = pipeline::plan(&info, max_height, audio_only)?;
    match &plan {
        Plan::Direct(f) => println!("  MP4 directo {}p {} (itag {})", f.height, f.codec, f.itag),
        Plan::Extract(f) if f.codec == "sound" => println!("  Audio original del servicio"),
        Plan::Extract(_) if matches!(output, Output::Mp3(_) | Output::Wav | Output::Flac) && pipeline::original_sound(&info).is_some() => {
            println!("  Sonido original del vídeo (si no sirve, el audio del MP4)")
        }
        Plan::Extract(f) => println!("  Audio AAC del MP4 de {}p", f.height),
        Plan::Merge { video: Some(v), .. } => {
            let fps = if v.fps > 30 { v.fps.to_string() } else { String::new() };
            println!("  Vídeo {}p{fps} {}  +  audio AAC", v.height, v.codec);
        }
        Plan::Merge { video: None, .. } => println!("  Solo audio AAC"),
    }

    let out = opts.output.clone().unwrap_or_else(|| pipeline::default_output(&info, &opts.dir, output.extension(audio_only)));
    let (video_bar, audio_bar) = (Progress::new("Vídeo"), Progress::new("Audio"));
    let on_progress = |stage: Stage, done: u64, total: u64| match stage {
        Stage::Video => video_bar.update(done, total),
        Stage::Audio => audio_bar.update(done, total),
        Stage::Muxing => eprintln!("  Uniendo..."),
    };
    let tags = audio_only.then(|| pipeline::info_tags(agent, &info));
    pipeline::execute(agent, &info, &plan, &out, Target { output, tags: tags.as_ref() }, &on_progress, &AtomicBool::new(false))?;

    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!("Guardado: {}  ({} MB)", out.display(), mb(size));
    Ok(())
}

fn catalog_one(agent: &ureq::Agent, opts: &Options, catalog: &dyn Catalog, kind: SpotKind, id: &str) -> Res<()> {
    let col = catalog.fetch(agent, kind, id)?;
    println!("{}  ({} canciones)", col.name, col.tracks.len());
    if opts.list {
        for (i, t) in col.tracks.iter().enumerate() {
            let s = t.duration_ms / 1000;
            println!("  {:>3}. {}  [{}:{:02}]", i + 1, t.label(), s / 60, s % 60);
        }
        return Ok(());
    }

    // Los catálogos son solo música: si no se pidió un formato de audio, M4A
    let format = if opts.format == "mp4" { Output::Mp4 } else { output_of(opts)?.0 };
    let dir = if kind == SpotKind::Track { opts.dir.clone() } else { opts.dir.join(pipeline::safe_name(&col.name)) };
    let total = col.tracks.len().min(opts.limit.unwrap_or(usize::MAX));
    let (mut failed, cancel) = (0, AtomicBool::new(false));
    for (i, t) in col.tracks.iter().take(total).enumerate() {
        let t = if kind == SpotKind::Track { t.clone() } else { catalog.refine(agent, t) };
        print!("[{}/{}] {} ... ", i + 1, total, t.label());
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let output = (kind == SpotKind::Track).then_some(opts.output.as_deref()).flatten();
        match pipeline::download_spotify_track(agent, &t, &dir, output, format, &|_, _, _| {}, &cancel) {
            Ok((path, m)) => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!("✓ {} MB  (coincidencia {:.0}%: «{}»)", mb(size), m.score, m.title);
            }
            Err(e) => {
                failed += 1;
                println!("✗ {e}");
            }
        }
    }
    if kind != SpotKind::Track {
        println!("Carpeta: {}", dir.display());
    }
    if failed > 0 {
        return Err(format!("{failed} de {total} canciones no se pudieron descargar").into());
    }
    Ok(())
}

fn main() {
    let opts = match parse_args() {
        Ok(Some(o)) if !o.inputs.is_empty() => o,
        Ok(other) => {
            println!("{HELP}");
            std::process::exit(if other.is_none() { 0 } else { 1 });
        }
        Err(e) => {
            eprintln!("Error: {e}\n\n{HELP}");
            std::process::exit(2);
        }
    };
    if opts.output.is_some() && opts.inputs.len() > 1 {
        eprintln!("Error: --output solo se puede usar con un vídeo");
        std::process::exit(2);
    }

    let agent = new_agent();

    let mut failed = 0;
    for input in &opts.inputs {
        let result = match sources::find_catalog(input) {
            Some((catalog, kind, id)) => catalog_one(&agent, &opts, catalog, kind, &id),
            None => fetch_one(&agent, &opts, input),
        };
        if let Err(e) = result {
            failed += 1;
            eprintln!("Error ({input}): {e}");
        }
    }
    std::process::exit(if failed > 0 { 1 } else { 0 });
}
