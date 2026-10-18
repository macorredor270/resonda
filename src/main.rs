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

#[derive(Debug, PartialEq)]
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

/// Qué se pidió en la línea de comandos.
#[derive(Debug, PartialEq)]
enum Command {
    Run(Options),
    Help,
    Version,
    Services,
}

const FORMATS: [&str; 6] = ["mp4", "mp3", "flac", "wav", "m4a", "aac"];

/// Lee los argumentos sin tocar la red ni terminar el proceso, y rechaza ya lo que no tiene sentido
/// (formato, calidad, número de canciones...), antes de descargar nada.
fn parse_args(args: impl IntoIterator<Item = String>) -> Res<Command> {
    let mut o = Options { quality: "1080".into(), output: None, dir: ".".into(), format: "mp4".into(), bitrate: 0, list: false, limit: None, inputs: vec![] };
    let mut args = args.into_iter();
    let mut any = false;
    while let Some(arg) = args.next() {
        any = true;
        // Admite "--opcion=valor"
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Res<String> {
            let v = inline.clone().or_else(|| args.next()).ok_or_else(|| format!("Falta el valor de {name}"))?;
            if v.is_empty() {
                return Err(format!("El valor de {name} está vacío").into());
            }
            Ok(v)
        };
        match flag.as_str() {
            "-q" | "--quality" => o.quality = value("--quality")?,
            "-o" | "--output" => o.output = Some(value("--output")?.into()),
            "-d" | "--dir" => o.dir = value("--dir")?.into(),
            "-a" | "--audio" => o.format = "m4a".into(),
            "-f" | "--format" => o.format = value("--format")?.to_lowercase(),
            "-b" | "--bitrate" => o.bitrate = value("--bitrate")?.parse().map_err(|_| "El valor de --bitrate debe ser un número")?,
            "-l" | "--list" => o.list = true,
            "-n" | "--limit" => o.limit = Some(value("--limit")?.parse().map_err(|_| "El valor de --limit debe ser un número")?),
            "-S" | "--services" => return Ok(Command::Services),
            "-V" | "--version" => return Ok(Command::Version),
            "-h" | "--help" => return Ok(Command::Help),
            f if f.starts_with('-') && f.len() > 1 => return Err(format!("Opción desconocida: {f}").into()),
            _ => o.inputs.push(arg),
        }
    }
    if !FORMATS.contains(&o.format.as_str()) {
        return Err(format!("Formato no soportado: \"{}\" (usa mp4, mp3, flac, wav, m4a o aac)", o.format).into());
    }
    if o.quality != "best" && o.quality.parse::<u32>().ok().filter(|&h| h > 0).is_none() {
        return Err(format!("Calidad no válida: \"{}\" (un número de píxeles como 720, o \"best\")", o.quality).into());
    }
    if o.bitrate != 0 && ![128, 192, 256, 320].contains(&o.bitrate) {
        return Err("El valor de --bitrate debe ser 128, 192, 256 o 320".into());
    }
    if o.limit == Some(0) {
        return Err("El valor de --limit debe ser al menos 1".into());
    }
    if o.output.is_some() && o.inputs.len() > 1 {
        return Err("--output solo se puede usar con un enlace".into());
    }
    if o.inputs.is_empty() {
        return if any { Err("Falta el enlace".into()) } else { Ok(Command::Help) };
    }
    Ok(Command::Run(o))
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

    // La calidad ya se validó al leer los argumentos
    let max_height: u32 = if opts.quality == "best" { u32::MAX } else { opts.quality.parse().unwrap_or(1080) };

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
    let mut used = std::collections::HashSet::new();
    let ext = format.extension(true);
    for (i, t) in col.tracks.iter().take(total).enumerate() {
        let t = if kind == SpotKind::Track { t.clone() } else { catalog.refine(agent, t) };
        print!("[{}/{}] {} ... ", i + 1, total, t.label());
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let path = match (kind == SpotKind::Track).then_some(opts.output.clone()).flatten() {
            Some(p) => p,
            None => pipeline::unique_track_path(&dir, &t, ext, &mut used),
        };
        match pipeline::download_spotify_track(agent, &t, &dir, Some(&path), format, &|_, _, _| {}, &cancel) {
            Ok((path, m)) => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!("✓ {} MB  (coincidencia {:.0}%: «{}»)", mb(size), m.score, m.title);
            }
            Err(e) => {
                failed += 1;
                println!("✗ {}", resonda::explain(&*e));
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

fn print_services() {
    for e in EXTRACTORS {
        println!("{:<10} {}", e.id(), e.name());
    }
    for c in CATALOGS {
        println!("{:<10} {} (solo metadatos; el audio sale de YouTube)", c.id(), c.name());
    }
}

fn main() {
    // Un fallo interno no debe enseñar un volcado en inglés: se avisa de que es un error del programa
    std::panic::set_hook(Box::new(|info| {
        eprintln!("Error interno de Resonda: {info}\nSi puedes, cuéntalo en https://github.com/macorredor270/resonda/issues con el enlace que usaste.");
    }));

    let opts = match parse_args(std::env::args().skip(1)) {
        Ok(Command::Run(o)) => o,
        Ok(Command::Help) => {
            println!("{HELP}");
            std::process::exit(0);
        }
        Ok(Command::Version) => {
            println!("resonda {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        Ok(Command::Services) => {
            print_services();
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error: {e}\n\n{HELP}");
            std::process::exit(2);
        }
    };

    let agent = new_agent();

    let mut failed = 0;
    for input in &opts.inputs {
        let result = match sources::find_catalog(input) {
            Some((catalog, kind, id)) => catalog_one(&agent, &opts, catalog, kind, &id),
            None => fetch_one(&agent, &opts, input),
        };
        if let Err(e) = result {
            failed += 1;
            eprintln!("Error ({input}): {}", resonda::explain(&*e));
        }
    }
    std::process::exit(if failed > 0 { 1 } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Res<Command> {
        parse_args(s.split_whitespace().map(str::to_string))
    }

    fn run(s: &str) -> Options {
        match args(s).unwrap() {
            Command::Run(o) => o,
            other => panic!("se esperaba Run, salió {other:?}"),
        }
    }

    fn err(s: &str) -> String {
        args(s).unwrap_err().to_string()
    }

    #[test]
    fn valores_por_defecto() {
        let o = run("https://youtu.be/dQw4w9WgXcQ");
        assert_eq!((o.format.as_str(), o.quality.as_str(), o.bitrate, o.limit, o.list), ("mp4", "1080", 0, None, false));
        assert_eq!(o.inputs, ["https://youtu.be/dQw4w9WgXcQ"]);
    }

    #[test]
    fn opciones_cortas_largas_y_con_igual() {
        let a = run("-f mp3 -b 256 -q 720 -d salida -n 3 enlace");
        let b = run("--format=MP3 --bitrate=256 --quality=720 --dir=salida --limit=3 enlace");
        assert_eq!(a, b);
        assert_eq!((a.format.as_str(), a.bitrate, a.dir, a.limit), ("mp3", 256, PathBuf::from("salida"), Some(3)));
        assert_eq!(run("-a enlace").format, "m4a");
        assert_eq!(run("-q best enlace").quality, "best");
        assert!(run("-l enlace").list);
        assert_eq!(run("-o a.mp4 enlace").output, Some(PathBuf::from("a.mp4")));
    }

    #[test]
    fn varios_enlaces_y_ordenes_mezclados() {
        let o = run("uno -f flac dos --dir x tres");
        assert_eq!(o.inputs, ["uno", "dos", "tres"]);
    }

    #[test]
    fn comandos_sueltos() {
        assert_eq!(args("-h").unwrap(), Command::Help);
        assert_eq!(args("--help enlace").unwrap(), Command::Help);
        assert_eq!(args("-V").unwrap(), Command::Version);
        assert_eq!(args("--services").unwrap(), Command::Services);
        assert_eq!(args("").unwrap(), Command::Help);
    }

    #[test]
    fn rechaza_lo_que_no_tiene_sentido_antes_de_usar_la_red() {
        assert!(err("--formato mp3 x").contains("Opción desconocida"));
        assert!(err("x -f").contains("Falta el valor de --format"));
        assert!(err("x -f ogg").contains("Formato no soportado"));
        assert!(err("x -q 0").contains("Calidad no válida"));
        assert!(err("x -q -5").contains("Calidad no válida"));
        assert!(err("x -q alta").contains("Calidad no válida"));
        assert!(err("x -b 100").contains("128, 192, 256 o 320"));
        assert!(err("x -b mucho").contains("debe ser un número"));
        assert!(err("x -n 0").contains("al menos 1"));
        assert!(err("x -n -1").contains("debe ser un número"));
        assert!(err("x --dir=").contains("vacío"));
        assert!(err("a b -o salida.mp4").contains("solo se puede usar con un enlace"));
    }

    #[test]
    fn sin_enlace_es_un_error_y_sin_nada_es_la_ayuda() {
        assert!(err("-f mp3").contains("Falta el enlace"));
        assert!(err("-l").contains("Falta el enlace"));
        assert_eq!(run("-l -f flac enlace").inputs.len(), 1);
    }

    #[test]
    fn entradas_raras_no_hacen_panico() {
        for input in ["-", "--", "=", "--=", "-=", "--format==mp3", "é", "\u{0}", "-f=mp3", "--limit=99999999999999999999"] {
            let _ = parse_args([input.to_string()]);
            let _ = parse_args([input.to_string(), "x".to_string()]);
        }
    }
}
