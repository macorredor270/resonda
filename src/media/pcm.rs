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
