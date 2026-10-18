<p align="center">
  <img src="docs/banner.svg" alt="Resonda: descarga vídeos y música. Código propio, 100 % libre." width="100%">
</p>

<p align="center">
  <b>Descarga vídeos y música de YouTube y Spotify</b> a MP4, MP3, FLAC, WAV, M4A o AAC.<br>
  Código propio · 100 % libre (MIT) · sin yt-dlp ni ffmpeg
</p>

<p align="center">
  <a href="https://github.com/macorredor270/resonda/releases/tag/v1.0.0"><img alt="Release" src="https://img.shields.io/github/v/release/macorredor270/resonda?label=versi%C3%B3n&color=00d4ff"></a>
  <a href="LICENSE"><img alt="Licencia" src="https://img.shields.io/badge/licencia-MIT-00d4ff"></a>
  <img alt="Lenguaje" src="https://img.shields.io/badge/rust-stable-00d4ff">
  <img alt="Dependencias de descarga" src="https://img.shields.io/badge/yt--dlp%20%2F%20ffmpeg-cero-4cc38a">
</p>

<p align="center">
  <a href="https://github.com/macorredor270/resonda/releases/latest"><b>⬇ Descargas</b></a> ·
  <a href="#libre-de-verdad">Libre</a> ·
  <a href="#todo-hecho-aquí">Todo hecho aquí</a> ·
  <a href="#librerías-que-usamos">Librerías</a> ·
  <a href="#compilar-y-usar">Compilar</a>
</p>

---

## Descargas

Todo lo compilado está en la página de [**Releases**](https://github.com/macorredor270/resonda/releases/latest):

| Sistema | Archivo | Qué es |
|---|---|---|
| **Linux, cualquier distro** | `Resonda-*-x86_64.AppImage` | La ventana, portable (lleva Qt dentro). `chmod +x` y a ejecutar |
| **Debian, Ubuntu y derivadas** | `resonda_*_amd64.deb` | Terminal y ventana. `sudo apt install ./resonda_*_amd64.deb` |
| **Linux, solo terminal** | `resonda-cli-*-linux-x86_64.tar.gz` | Un único binario, sin dependencias |
| **Windows** | `Resonda-*-windows-x64.zip` | `resonda.exe` (terminal) y `resonda-gui.exe` (ventana) |
| **macOS** (Apple Silicon e Intel) | `resonda-cli-*-macos-arm64.tar.gz` / `-macos-x86_64.tar.gz` | Solo terminal (la ventana aún no se empaqueta para Mac) |
| **Linux ARM** (Raspberry Pi 4/5, servidores) | `resonda-cli-*-linux-aarch64.tar.gz` | Solo terminal |

Cada versión incluye `SHA256SUMS.txt` para comprobar los archivos. Los paquetes se compilan en GitHub Actions
(Ubuntu 22.04 y Windows), sin nada oculto: el flujo está en [`.github/workflows/release.yml`](.github/workflows/release.yml).

## Libre de verdad

- **Licencia [MIT](LICENSE)**: úsalo, cópialo, modifícalo, véndelo. No hace falta pedir permiso.
- **Sin cuentas, sin anuncios, sin telemetría.** La app solo habla con los servicios que usas: YouTube (`www.youtube.com`, sus servidores
  de vídeo y las miniaturas), TikTok (`www.tiktok.com` y sus servidores de vídeo), Twitch (`gql.twitch.tv` y sus servidores de clips),
  Bandcamp, y, para leer metadatos, Spotify (`open.spotify.com`), Deezer (`api.deezer.com`), Apple Music
  (`itunes.apple.com`) y Tidal (`tidal.com`), además de las imágenes de portada. Respeta `HTTPS_PROXY` / `ALL_PROXY` si los tienes definidos.
- **Sin nada oculto**: son unas 5 000 líneas y caben en una tarde de lectura.

## Todo hecho aquí

Resonda **no usa ni incluye yt-dlp, youtube-dl, ffmpeg, spotDL ni ninguna otra herramienta de descarga**, y no lanza
ningún programa externo (compruébalo: `grep -rn "Command::new" src gui/src` no encuentra nada).
Lo que hace la app es código nuestro:

| Pieza | Qué hace |
|---|---|
| **Extractores** | Un módulo por servicio ([`src/sources/`](src/sources)): reconoce los enlaces y entrega los formatos. Hoy YouTube, TikTok, Twitch y Bandcamp (y los catálogos de Spotify, Deezer, Apple Music y Tidal); añadir otro es [implementar un trait](#añadir-un-servicio) |
| **Cliente de YouTube** | Abre una sesión anónima, elige el cliente que sirve el vídeo completo y busca vídeos |
| **Descargador** | Por trozos, en paralelo, con reintentos y cancelación limpia |
| **Muxer MP4** | Lee los MP4 fragmentados de YouTube y escribe un MP4 normal con vídeo y audio |
| **Extractor AAC** | Saca el audio a `.aac` (ADTS) sin recodificar |
| **Codificador FLAC** | Sin pérdida, con predictores fijos y decorrelación estéreo |
| **Escritor WAV** | PCM de 16 bits |
| **Etiquetas** | Título, artista, álbum, año y portada en MP4/M4A, MP3 (ID3v2), FLAC y WAV |
| **Spotify → YouTube** | Lee los datos de un enlace y elige la grabación que encaja (duración, título, artista) |
| **Ventana** | Interfaz Qt/QML con selector de servicios, formatos y progreso |

## Librerías que usamos

Solo cinco dependencias directas (unos 45 crates en total en Linux, casi todos TLS y utilidades) y Qt para la ventana:

| Librería | Para qué | Licencia |
|---|---|---|
| [`ureq`](https://crates.io/crates/ureq) | HTTPS (con `rustls`, sin OpenSSL) | MIT / Apache-2.0 |
| [`serde_json`](https://crates.io/crates/serde_json) | Leer las respuestas JSON | MIT / Apache-2.0 |
| [`symphonia-codec-aac`](https://crates.io/crates/symphonia-codec-aac), [`symphonia-bundle-mp3`](https://crates.io/crates/symphonia-bundle-mp3) y `symphonia-core` | **Solo** decodificar AAC y MP3 (para WAV, FLAC y MP3) | MPL-2.0 |
| [`rusty_mp3`](https://crates.io/crates/rusty_mp3) | Codificar MP3 | Apache-2.0 |
| [Qt 6](https://www.qt.io) (Quick, QuickControls2, Svg) | La ventana. Se enlaza dinámicamente | LGPL-3.0 |
| [Simple Icons](https://simpleicons.org) | Logos de los servicios | CC0-1.0 |

Los paquetes AppImage, `.deb` y Windows llevan Qt sin modificar; para cambiarlo basta con sustituir sus bibliotecas.
Los logos son marcas registradas de sus dueños: se usan solo para identificar cada servicio y Resonda no está afiliada
a ninguno. Puedes ver todas las dependencias con `cargo tree`.

### De dónde viene el conocimiento

Para entender cómo funcionan los clientes de YouTube estudiamos [yt-dlp](https://github.com/yt-dlp/yt-dlp)
(Unlicense) y para la idea de emparejar canciones de Spotify con vídeos, [spotDL](https://github.com/spotDL/spotify-downloader)
(MIT). Gracias a ambos proyectos. La lógica se reescribió desde cero en Rust: aquí no hay código suyo.

## Servicios y formatos

| Servicio | Enlaces | MP4 | M4A · AAC | MP3 | FLAC · WAV |
|---|---|---|---|---|---|
| **YouTube** | vídeos, Shorts, `youtu.be`, `music.youtube.com`, ID suelto | H.264 hasta 1080p, AV1 por encima | original | recodificado (128–320 kbps) | decodificados |
| **TikTok** | `tiktok.com/@usuario/video/…`, `vm.` / `vt.tiktok.com/…` | H.264 sin marca de agua | original (HE-AAC) | sonido original, sin recodificar¹ | calidad completa¹ |
| **Twitch** | clips: `clips.twitch.tv/…`, `twitch.tv/canal/clip/…` (no VODs ni directos) | 360p a 1080p | original | recodificado | decodificados |
| **Bandcamp** | una canción (`canal.bandcamp.com/track/…`); los álbumes piden el enlace de una canción | — (solo audio) | no² | MP3 de 128 kbps, sin recodificar | decodificados |
| **Spotify** · **Deezer** · **Apple Music** · **Tidal** | canción, álbum o playlist (Apple Music: canción o álbum; Tidal: solo canción) | — | sí³ | sí³ | sí³ |

¹ TikTok guarda el audio de sus vídeos en **HE-AAC** (SBR), que Resonda no sabe decodificar entero
(no hay decodificador SBR libre en Rust). Para MP3, FLAC y WAV usa el *sonido original* que TikTok
publica aparte (MP3 de 128 kbps o AAC-LC, calidad completa; Resonda decodifica los dos). Si el vídeo lleva música de otra persona
y no es sonido original, esa pista no es el audio del vídeo: entonces solo se puede decodificar el núcleo
del HE-AAC, que sale mono/22 kHz y sin agudos, y la terminal avisa. **M4A y AAC conservan siempre la
calidad original.** Por compatibilidad, el MP4 de TikTok es H.264 aunque haya una versión HEVC más grande.

² Bandcamp entrega MP3; pasarlo a M4A o AAC exigiría recodificarlo, y Resonda no tiene codificador AAC.
³ De estos cuatro catálogos solo se leen los **metadatos** (título, artista, duración, portada). Su audio
está protegido con DRM y no se toca: cada canción se busca y se descarga desde YouTube, y se guarda con sus etiquetas.

`resonda -S` lista los servicios disponibles.

## Formatos

| Formato | Cómo se obtiene |
|---|---|
| **MP4** | Vídeo y audio unidos con nuestro muxer |
| **M4A · AAC** | El audio de YouTube tal cual, sin recodificar |
| **WAV · FLAC** | Decodificando el AAC (el escritor WAV y el codificador FLAC son nuestros) |
| **MP3** | Decodificando el AAC y codificando con `rusty_mp3` (128 a 320 kbps) |

## Compilar y usar

```sh
make cli        # terminal:  target/release/resonda
make gui        # ventana:   gui/build/resonda-gui   (make run la abre)
make test       # tests del núcleo
make clean      # borra lo compilado: el proyecto vuelve a ocupar unos 400 KB
```

Requisitos: Rust; para la ventana, Qt 6.5 o superior (Quick, QuickControls2 y Svg), CMake y Ninja.

### macOS y Windows

La terminal compila igual en los dos con Rust a secas (`cargo build --release`). En macOS,
`brew install rust` basta; para la ventana añade `brew install qt cmake ninja` y
`cmake -S gui -B gui/build -G Ninja -DCMAKE_PREFIX_PATH="$(brew --prefix qt)"`. En Windows se
usa la cadena de herramientas MSVC (en PowerShell, no en el bash de Git: `link.exe` es otro). Las
pruebas (`cargo test`) corren en CI en Linux x86_64 y ARM, Windows y macOS.

### Compilar en Arch y otras distros

Los paquetes de [Releases](#descargas) (`.deb`, AppImage, `.exe`) cubren Debian/Ubuntu, cualquier
distro vía AppImage y Windows. Para Arch (y en general cualquier distro sin paquete propio) se
compila desde el código — es rápido, `resonda -f mp3 -b 256 <enlace>` está listo a los pocos
segundos de tener los requisitos instalados.

**Arch / Manjaro / EndeavourOS:**

```sh
sudo pacman -S --needed rust qt6-base qt6-declarative qt6-svg cmake ninja
git clone https://github.com/macorredor270/resonda.git && cd resonda
make cli && make gui
```

**Fedora:**

```sh
sudo dnf install rust cargo qt6-qtbase-devel qt6-qtdeclarative-devel qt6-qtsvg-devel cmake ninja-build
```

**openSUSE:**

```sh
sudo zypper install rust cargo qt6-base-devel qt6-declarative-devel qt6-svg-devel cmake ninja
```

**Debian / Ubuntu (compilando en vez de usar el `.deb`):**

```sh
sudo apt install cargo qt6-base-dev qt6-declarative-dev libqt6svg6-dev cmake ninja-build
```

Solo hace falta Qt para `make gui`; `make cli` (la terminal) compila con Rust a secas, sin ninguna
dependencia del sistema más allá del propio compilador. Si `cmake` no encuentra Qt, exporta
`CMAKE_PREFIX_PATH` apuntando a su instalación (p. ej. Qt instalado a mano en `~/Qt/6.x/gcc_64`).

```sh
resonda -f mp3 -b 256 <enlace>      # YouTube, TikTok o Spotify (canción, álbum o playlist)
resonda -f m4a <enlace de TikTok>   # el audio original de un vídeo de TikTok
resonda -f flac <canción de Bandcamp>
resonda -S                          # servicios soportados
resonda -q 1080 <enlace>            # vídeo MP4 a 1080p
resonda --help
resonda-gui [enlace]
```

Para generar los paquetes de Linux: `packaging/build-linux.sh` (deja `.tar.gz`, `.deb` y AppImage en `dist/`).

## Pruebas

`make test` (o `cargo test --release`) ejecuta 90 pruebas que no necesitan internet. Lo que cubren:

- **Descarga contra un servidor HTTP falso** (`src/testutil.rs`) que se porta mal a propósito: reintenta los
  fallos pasajeros (500, 429, conexión cortada), no reintenta los definitivos (403, 404, 410, 416), no acepta
  nunca un trozo equivocado (servidor que ignora `Range`), respeta «cancelar» también durante las esperas,
  y comprueba que un fallo **no toca un archivo que ya existía ni deja `.part`** sueltos.
- **Archivos dañados**: MP4 (normales y fragmentados) truncados en todas las longitudes y con bytes cambiados,
  tablas incoherentes, MP3 corruptos o basura, y casos límite de WAV y FLAC (vacío, una muestra, número impar…):
  siempre error, nunca pánico.
- **Ida y vuelta de audio**: un tono se codifica a MP3 y se decodifica, y se comprueba formato, duración y nivel.
- **Páginas y JSON de cada servicio** con miles de variantes dañadas (recortes, comillas cambiadas, tipos
  inesperados), y 30 000 enlaces aleatorios, para que un cambio en un servicio dé un error claro y no un pánico.
- **Línea de comandos**: argumentos válidos y no válidos, rechazados antes de tocar la red.

Los servicios reales (YouTube, TikTok…) no se llaman en las pruebas porque cambian sin avisar; cada uno
tiene un ejemplo fijo de su página. Para comprobar un servicio de verdad, ejecuta la terminal con un enlace.

## Añadir un servicio

Cada servicio es un tipo que implementa `Extractor` ([`src/sources/mod.rs`](src/sources/mod.rs)):

```rust
pub struct MiRed;
impl Extractor for MiRed {
    fn id(&self) -> &'static str { "mired" }
    fn name(&self) -> &'static str { "MiRed" }
    fn matches(&self, input: &str) -> bool { on_domain(input, "mired.com") }   // sin red
    fn extract(&self, agent: &ureq::Agent, input: &str) -> Res<VideoInfo> { /* ... */ }
}
```

Los catálogos de solo metadatos (como Deezer) implementan `Catalog` en vez de `Extractor`. `extract` devuelve un `VideoInfo` con los formatos (`Kind::Muxed` para MP4 completos con vídeo y audio,
o `Video` + `Audio` por separado) y las cabeceras que el servicio exige al descargar (`Referer`, `Cookie`…).
Se añade a `EXTRACTORS` y la terminal, la API C y la ventana ya lo usan. [`tiktok.rs`](src/sources/tiktok.rs)
es el ejemplo más corto, con su prueba del parseo sin red. En la ventana, añade además la tarjeta del
servicio a `live` en `gui/qml/ServiceSetup.qml`. Los MP4 normales (no fragmentados) y los fragmentados se leen
con el mismo muxer.

## Estructura

```
.
├── Cargo.toml · Cargo.lock · Makefile · LICENSE
├── .github/workflows/           publicación (Linux, Windows, macOS y ARM) y pruebas
├── docs/                        banner del README
├── include/resonda.h            API C pública (la usa la ventana)
├── packaging/                   script de paquetes de Linux
├── src/                         núcleo en Rust
│   ├── main.rs                  línea de comandos
│   ├── lib.rs · ffi.rs · pipeline.rs · io.rs
│   ├── sources/                 youtube · tiktok · twitch · bandcamp · spotify · deezer · apple · tidal · matching · download
│   │                            (trait Extractor en mod.rs, trait Catalog en catalog.rs)
│   └── media/                   pcm · wav · flac · mp3 · tags
│       └── mp4/                 muxer MP4 (boxes, track, build, ilst, aac)
└── gui/                         ventana Qt6 / QML
    ├── src/                     capa C++ entre QML y el núcleo
    ├── qml/                     Main.qml · ServiceSetup.qml
    ├── assets/logos/            logos de los servicios (se incrustan en el binario)
    └── data/                    resonda.desktop · resonda.svg (icono)
```

## Aviso

Descargar contenido puede ir contra las condiciones de uso de YouTube y de Spotify, y el contenido que descargues tiene
sus propios derechos: úsalo con lo que sea tuyo o tengas permiso de guardar. De Spotify **solo se leen los datos**
(título, artista, portada); su audio, que está protegido con DRM, no se toca: cada canción se busca y se baja de YouTube.

## Licencia

[MIT](LICENSE) © 2026 Resonda contributors.
