// MP3: el codificador es `rusty_mp3` (Rust puro, sin dependencias); la etiqueta ID3v2 con la
// portada es código propio.

use crate::media::Tags;
use crate::media::pcm::{PcmFormat, PcmSink};
use crate::Res;
use rusty_mp3::{Mp3Encoder, Mp3EncoderConfig};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub const DEFAULT_KBPS: u32 = 192;

pub struct Mp3Writer {
    out: BufWriter<File>,
    tags: Option<Tags>,
    kbps: u32,
    enc: Option<Mp3Encoder>,
    format: Option<PcmFormat>,
}

impl Mp3Writer {
    pub fn create(path: &Path, tags: Option<&Tags>, kbps: u32) -> Res<Mp3Writer> {
        let kbps = if kbps == 0 { DEFAULT_KBPS } else { kbps };
        Ok(Mp3Writer { out: BufWriter::new(File::create(path)?), tags: tags.cloned(), kbps, enc: None, format: None })
    }

    fn drain(&mut self) -> Res<()> {
        if let Some(enc) = self.enc.as_mut() {
            while let Ok(packet) = enc.next_packet() {
                self.out.write_all(&packet)?;
            }
        }
        Ok(())
    }
}

fn synchsafe(n: usize) -> [u8; 4] {
    [(n >> 21 & 0x7f) as u8, (n >> 14 & 0x7f) as u8, (n >> 7 & 0x7f) as u8, (n & 0x7f) as u8]
}

/// Texto de un marco ID3v2.3: ISO-8859-1 si cabe, si no UTF-16 con BOM
fn text_payload(s: &str) -> Vec<u8> {
    if s.chars().all(|c| (c as u32) < 0x80) {
        let mut v = vec![0u8];
        v.extend(s.as_bytes());
        v
    } else {
        let mut v = vec![1u8, 0xff, 0xfe];
        for u in s.encode_utf16() {
            v.extend(u.to_le_bytes());
        }
        v
    }
}

fn frame(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut f = id.to_vec();
    f.extend((payload.len() as u32).to_be_bytes()); // en ID3v2.3 el tamaño no es "synchsafe"
    f.extend([0, 0]);
    f.extend(payload);
    f
}

/// Etiqueta ID3v2.3: título, artista, álbum, año y portada
pub fn id3v2(t: &Tags) -> Vec<u8> {
    let mut frames = frame(b"TIT2", &text_payload(&t.title));
    frames.extend(frame(b"TPE1", &text_payload(&t.artist)));
    if let Some(a) = &t.album {
        frames.extend(frame(b"TALB", &text_payload(a)));
    }
    if let Some(y) = &t.year {
        frames.extend(frame(b"TYER", &text_payload(y)));
    }
    if let Some(c) = &t.cover {
        let mime: &[u8] = if c.starts_with(&[0x89, b'P']) { b"image/png\0" } else { b"image/jpeg\0" };
        let mut p = vec![0u8];
        p.extend(mime);
        p.extend([3, 0]); // portada frontal, sin descripción
        p.extend(c);
        frames.extend(frame(b"APIC", &p));
    }
    let mut out = b"ID3\x03\x00\x00".to_vec();
    out.extend(synchsafe(frames.len()));
    out.extend(frames);
    out
}

/// Copia un MP3 ya codificado sin tocar el audio: le quita la etiqueta ID3v2 que traiga (si la
/// hay) y le pone la nuestra.
pub fn retag(input: &Path, dest: &Path, tags: Option<&Tags>) -> Res<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut src = File::open(input)?;
    let mut head = [0u8; 10];
    let n = src.read(&mut head)?;
    let mut skip = 0u64;
    if n == 10 && &head[0..3] == b"ID3" && head[6..10].iter().all(|b| b & 0x80 == 0) {
        let size = head[6..10].iter().fold(0u64, |acc, b| (acc << 7) | u64::from(*b));
        skip = 10 + size + if head[5] & 0x10 != 0 { 10 } else { 0 };
    }
    src.seek(SeekFrom::Start(skip))?;
    let mut out = BufWriter::new(File::create(dest)?);
    if let Some(t) = tags {
        out.write_all(&id3v2(t))?;
    }
    std::io::copy(&mut src, &mut out)?;
    out.flush()?;
    Ok(())
}

impl PcmSink for Mp3Writer {
    fn start(&mut self, f: PcmFormat) -> Res<()> {
        self.format = Some(f);
        if let Some(t) = &self.tags {
            self.out.write_all(&id3v2(t))?;
        }
        self.enc = Some(Mp3Encoder::new(Mp3EncoderConfig { bitrate_kbps: self.kbps, vbr_quality: None }));
        Ok(())
    }

    fn write(&mut self, samples: &[i16]) -> Res<()> {
        let f = self.format.ok_or("MP3 sin iniciar")?;
        self.enc
            .as_mut()
            .ok_or("MP3 sin iniciar")?
            .push_pcm_s16(samples, f.channels as u16, f.sample_rate)
            .map_err(|e| format!("Codificador MP3: {e:?}"))?;
        self.drain()
    }

    fn finish(&mut self) -> Res<()> {
        if let Some(enc) = self.enc.as_mut() {
            enc.finish();
        }
        self.drain()?;
        self.out.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retag_cambia_la_etiqueta_y_conserva_el_audio() {
        let dir = std::env::temp_dir().join(format!("resonda-retag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (i, o) = (dir.join("in.mp3"), dir.join("out.mp3"));
        let audio = [0xffu8, 0xfb, 0x90, 0x00, 1, 2, 3, 4];
        let old = Tags { title: "viejo".into(), ..Default::default() };
        std::fs::write(&i, [id3v2(&old), audio.to_vec()].concat()).unwrap();
        let new = Tags { title: "nuevo".into(), artist: "A".into(), ..Default::default() };
        retag(&i, &o, Some(&new)).unwrap();
        let out = std::fs::read(&o).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(out.ends_with(&audio));
        assert!(out.windows(5).any(|w| w == b"nuevo") && !out.windows(5).any(|w| w == b"viejo"));
    }

    #[test]
    fn etiqueta_id3_bien_formada() {
        let t = Tags { title: "Título".into(), artist: "Art".into(), album: Some("Alb".into()), year: Some("2001".into()), cover: Some(vec![0xff, 0xd8, 1, 2]) };
        let id3 = id3v2(&t);
        assert_eq!(&id3[0..3], b"ID3");
        let size = ((id3[6] as usize) << 21) | ((id3[7] as usize) << 14) | ((id3[8] as usize) << 7) | id3[9] as usize;
        assert_eq!(size, id3.len() - 10);
        for f in ["TIT2", "TPE1", "TALB", "TYER", "APIC"] {
            assert!(id3.windows(4).any(|w| w == f.as_bytes()), "falta {f}");
        }
    }
}
