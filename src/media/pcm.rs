// Decodifica el AAC de YouTube a PCM de 16 bits entrelazado. Es lo único que usa una librería
// ajena (symphonia, solo el decodificador AAC); todo lo que se hace con el PCM es código propio.

use crate::media::container::{sniff, Container};
use crate::media::mp4::AacStream;
use crate::Res;
use std::path::Path;
use symphonia_bundle_mp3::{MpaDecoder, MpaReader};
use symphonia_codec_aac::AacDecoder;
use symphonia_core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia_core::codecs::CodecParameters;
use symphonia_core::formats::{FormatOptions, FormatReader};
use symphonia_core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia_core::packet::PacketRef;
use symphonia_core::units::{Duration, Timestamp};

#[derive(Clone, Copy, Debug)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: u8,
}

/// Destino del PCM (WAV, FLAC, MP3...). Recibe las muestras según se decodifican, así que un
/// vídeo de horas no se carga entero en memoria.
pub trait PcmSink {
    /// Se llama una vez, antes de la primera muestra.
    fn start(&mut self, format: PcmFormat) -> Res<()>;
    /// Muestras `i16` entrelazadas (L R L R...).
    fn write(&mut self, samples: &[i16]) -> Res<()>;
    /// Se llama al terminar, para cerrar el archivo (cabeceras, último bloque...).
    fn finish(&mut self) -> Res<()>;
}

/// Decodifica un M4A con audio AAC-LC y lo vuelca en `sink`.
pub fn decode_aac_to(input: &Path, sink: &mut dyn PcmSink) -> Res<()> {
    let aac = AacStream::open(input)?;
    if aac.channels > 2 {
        return Err(format!("Audio de {} canales no soportado (solo mono o estéreo)", aac.channels).into());
    }
    if aac.he_aac {
        eprintln!(
            "Aviso: este audio es HE-AAC (como el de TikTok). Resonda no incluye decodificador SBR, así que WAV, FLAC y MP3 \
             salen sin los agudos (~{} kHz máx.). M4A y AAC conservan la calidad original.",
            aac.sample_rate / 2000
        );
    }
    let mut params = AudioCodecParameters::new();
    params.for_codec(CODEC_ID_AAC).with_sample_rate(aac.sample_rate).with_extra_data(aac.asc.clone().into_boxed_slice());
    let mut decoder = AacDecoder::try_new(&params, &AudioDecoderOptions::default()).map_err(|e| format!("Decodificador AAC: {e}"))?;

    sink.start(PcmFormat { sample_rate: aac.sample_rate, channels: aac.channels })?;
    let (mut pts, mut pcm) = (0i64, Vec::<i16>::new());
    aac.for_each_frame(|frame| {
        let packet = PacketRef::new(0, Timestamp::new(pts), Duration::new(1024), frame);
        pts += 1024;
        match decoder.decode_ref(&packet) {
            Ok(buf) => {
                buf.copy_to_vec_interleaved::<i16>(&mut pcm);
                sink.write(&pcm)
            }
            // Un fotograma dañado no debe tirar toda la canción: se sustituye por silencio
            Err(_) => {
                let silence = vec![0i16; 1024 * aac.channels as usize];
                sink.write(&silence)
            }
        }
    })?;
    sink.finish()
}

/// Decodifica un archivo de audio (M4A/MP4 con AAC, o MP3) y lo vuelca en `sink`. El formato se
/// reconoce por los bytes del archivo, no por su extensión.
pub fn decode_to(input: &Path, sink: &mut dyn PcmSink) -> Res<()> {
    match sniff(input) {
        Container::Mp3 => decode_mp3_to(input, sink),
        Container::Mp4 => decode_aac_to(input, sink),
        Container::Unknown => Err("Formato de audio desconocido: no es MP3 ni M4A".into()),
    }
}

/// Decodifica un MP3 y lo vuelca en `sink`.
pub fn decode_mp3_to(input: &Path, sink: &mut dyn PcmSink) -> Res<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(input)?;
    // El lector de MP3 empieza en el primer fotograma: se salta la etiqueta ID3v2 si la hay
    let mut head = [0u8; 10];
    let n = file.read(&mut head)?;
    let skip = if n == 10 && &head[0..3] == b"ID3" && head[6..10].iter().all(|b| b & 0x80 == 0) {
        10 + head[6..10].iter().fold(0u64, |acc, b| (acc << 7) | u64::from(*b)) + if head[5] & 0x10 != 0 { 10 } else { 0 }
    } else {
        0
    };
    file.seek(SeekFrom::Start(skip))?;

    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut reader = MpaReader::try_new(mss, FormatOptions::default()).map_err(|e| format!("MP3 ilegible: {e}"))?;
    let params = match reader.tracks().first().and_then(|t| t.codec_params.as_ref()) {
        Some(CodecParameters::Audio(p)) => p.clone(),
        _ => return Err("El MP3 no tiene audio".into()),
    };
    let mut decoder = MpaDecoder::try_new(&params, &AudioDecoderOptions::default()).map_err(|e| format!("Decodificador MP3: {e}"))?;

    let (mut started, mut pcm) = (false, Vec::<i16>::new());
    while let Some(packet) = reader.next_packet().map_err(|e| format!("MP3 dañado: {e}"))? {
        let Ok(buf) = decoder.decode(&packet) else { continue }; // un fotograma dañado se salta
        if !started {
            let spec = buf.spec();
            sink.start(PcmFormat { sample_rate: spec.rate(), channels: spec.channels().count().min(2) as u8 })?;
            started = true;
        }
        buf.copy_to_vec_interleaved::<i16>(&mut pcm);
        sink.write(&pcm)?;
    }
    if !started {
        return Err("El MP3 no tiene audio decodificable".into());
    }
    sink.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::flac::FlacWriter;
    use crate::media::mp3::Mp3Writer;
    use crate::media::wav::WavWriter;
    use crate::testutil::Rng;
    use std::path::PathBuf;

    /// Sumidero que se queda con lo que recibe.
    #[derive(Default)]
    struct Collect {
        format: Option<(u32, u8)>,
        samples: Vec<i16>,
        finished: bool,
    }

    impl PcmSink for Collect {
        fn start(&mut self, f: PcmFormat) -> Res<()> {
            self.format = Some((f.sample_rate, f.channels));
            Ok(())
        }
        fn write(&mut self, s: &[i16]) -> Res<()> {
            self.samples.extend_from_slice(s);
            Ok(())
        }
        fn finish(&mut self) -> Res<()> {
            self.finished = true;
            Ok(())
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("resonda-pcm-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Un tono de 440 Hz entrelazado.
    fn sine(rate: u32, channels: u8, secs: f64) -> Vec<i16> {
        let n = (rate as f64 * secs) as usize;
        (0..n).flat_map(|i| std::iter::repeat_n((f64::sin(i as f64 * 440.0 * std::f64::consts::TAU / rate as f64) * 12_000.0) as i16, channels as usize)).collect()
    }

    fn encode_mp3(path: &Path, samples: &[i16], rate: u32, channels: u8, kbps: u32) {
        let mut w = Mp3Writer::create(path, None, kbps).unwrap();
        w.start(PcmFormat { sample_rate: rate, channels }).unwrap();
        for block in samples.chunks(4096 * channels as usize) {
            w.write(block).unwrap();
        }
        w.finish().unwrap();
    }

    fn rms(s: &[i16]) -> f64 {
        (s.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / s.len().max(1) as f64).sqrt()
    }

    #[test]
    fn mp3_ida_y_vuelta_conserva_el_formato_y_la_senal() {
        for (rate, channels) in [(44100, 2u8), (44100, 1), (48000, 2)] {
            let d = dir("mp3rt");
            let path = d.join("t.mp3");
            let input = sine(rate, channels, 1.0);
            encode_mp3(&path, &input, rate, channels, 128);
            let mut sink = Collect::default();
            decode_to(&path, &mut sink).unwrap();
            assert!(sink.finished);
            let (r, c) = sink.format.unwrap();
            assert_eq!((r, c), (rate, channels), "formato");
            // El codificador añade relleno al principio y al final: la duración sale muy parecida
            let (got, want) = (sink.samples.len() as f64, input.len() as f64);
            assert!((got - want).abs() / want < 0.15, "{rate}/{channels}: {got} muestras, se esperaban {want}");
            // y no es silencio: el tono sigue ahí con un volumen parecido
            let level = rms(&sink.samples[sink.samples.len() / 4..sink.samples.len() * 3 / 4]);
            assert!((level - rms(&input)).abs() < 2500.0, "nivel {level}");
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn mp3_con_etiqueta_id3_delante_tambien_se_decodifica() {
        let d = dir("mp3id3");
        let (bare, tagged) = (d.join("a.mp3"), d.join("b.mp3"));
        encode_mp3(&bare, &sine(44100, 2, 0.5), 44100, 2, 128);
        let tags = crate::media::Tags { title: "Título".into(), artist: "Yo".into(), cover: Some(vec![0xff, 0xd8, 1, 2, 3]), ..Default::default() };
        crate::media::mp3::retag(&bare, &tagged, Some(&tags)).unwrap();
        let (mut a, mut b) = (Collect::default(), Collect::default());
        decode_to(&bare, &mut a).unwrap();
        decode_to(&tagged, &mut b).unwrap();
        assert_eq!(a.samples, b.samples);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn mp3_corrupto_o_basura_da_error_y_no_panico() {
        let d = dir("mp3fuzz");
        let path = d.join("t.mp3");
        encode_mp3(&path, &sine(44100, 2, 0.5), 44100, 2, 128);
        let good = std::fs::read(&path).unwrap();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut cases: Vec<Vec<u8>> = vec![vec![], vec![0xff], vec![0xff, 0xfb, 0x90], b"ID3".to_vec(), b"ID3\x04\0\0\x7f\x7f\x7f\x7f".to_vec()];
        cases.extend((0..good.len()).step_by(61).map(|n| good[..n].to_vec())); // truncados
        for _ in 0..150 {
            let mut bad = good.clone();
            for _ in 0..1 + rng.below(8) {
                let i = rng.below(bad.len());
                bad[i] = rng.next() as u8;
            }
            cases.push(bad);
        }
        for _ in 0..40 {
            // basura con muchos "falsos comienzos" de fotograma
            cases.push((0..2000).map(|i| if i % 7 == 0 { 0xff } else if i % 7 == 1 { 0xfb } else { rng.next() as u8 }).collect());
        }
        for (n, bytes) in cases.iter().enumerate() {
            std::fs::write(&path, bytes).unwrap();
            let mut sink = Collect::default();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode_to(&path, &mut sink)));
            assert!(r.is_ok(), "pánico decodificando el caso {n} ({} bytes)", bytes.len());
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn un_archivo_que_no_es_audio_se_rechaza() {
        let d = dir("unknown");
        let path = d.join("x.bin");
        for bytes in [&b""[..], b"hola mundo, esto no es audio", &[0u8; 64][..]] {
            std::fs::write(&path, bytes).unwrap();
            assert!(decode_to(&path, &mut Collect::default()).unwrap_err().to_string().contains("desconocido"));
        }
        assert!(decode_to(&d.join("no-existe"), &mut Collect::default()).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn wav_y_flac_aguantan_los_casos_limite() {
        let d = dir("edge");
        let cases: [(&str, u8, Vec<i16>); 7] = [
            ("vacio", 2, vec![]),
            ("una_muestra_mono", 1, vec![123]),
            ("un_fotograma_estereo", 2, vec![1, -1]),
            ("extremos", 2, [i16::MIN, i16::MAX, i16::MAX, i16::MIN].repeat(3000)),
            ("silencio", 2, vec![0; 20_000]),
            ("mitad_de_fotograma", 2, vec![5, 6, 7]), // número impar en estéreo
            ("ruido", 2, { let mut r = Rng(7); (0..30_000).map(|_| r.next() as i16).collect() }),
        ];
        for (name, channels, samples) in cases {
            for kind in ["wav", "flac"] {
                let path = d.join(format!("{name}.{kind}"));
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Res<()> {
                    let mut sink: Box<dyn PcmSink> = if kind == "wav" { Box::new(WavWriter::create(&path, None)?) } else { Box::new(FlacWriter::create(&path, None)?) };
                    sink.start(PcmFormat { sample_rate: 44100, channels })?;
                    for block in samples.chunks(1000) {
                        sink.write(block)?;
                    }
                    sink.finish()
                }));
                assert!(r.is_ok(), "pánico con {name}.{kind}");
                if let Ok(Ok(())) = r {
                    let bytes = std::fs::read(&path).unwrap();
                    assert!(bytes.starts_with(if kind == "wav" { b"RIFF" } else { b"fLaC" }), "{name}.{kind}");
                }
            }
        }
        let _ = std::fs::remove_dir_all(d);
    }
}
