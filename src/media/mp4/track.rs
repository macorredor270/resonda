//! Análisis de una pista de un MP4: fragmentado (moov + moof/mdat, como sirve YouTube) o normal
//! (tablas stbl completas en el moov, como sirven TikTok y la mayoría de las redes).

use super::boxes::*;
use crate::Res;
use std::fs::File;
use std::path::{Path, PathBuf};

// Una hora de vídeo a 60 fps son ~216.000 muestras; esto solo corta valores absurdos.
pub(super) const MAX_SAMPLES_PER_RUN: u32 = 1 << 24;

// ---------- análisis de una pista fragmentada ----------

pub(super) struct Sample {
    pub(super) dur: u32,
    pub(super) size: u32,
    pub(super) cto: i32,
    pub(super) sync: bool,
}

pub(super) struct Chunk {
    pub(super) src: u64,
    pub(super) len: u64,
    pub(super) count: u32,
    pub(super) time: f64,
    pub(super) rel: u64,
    pub(super) out: u64,
}

pub(super) struct Track {
    pub(super) path: PathBuf,
    pub(super) moov: Vec<u8>,
    pub(super) trak: BoxNode,
    pub(super) stsd: BoxNode,
    pub(super) timescale: u32,
    pub(super) fourcc: [u8; 4],
    pub(super) samples: Vec<Sample>,
    pub(super) chunks: Vec<Chunk>,
    pub(super) duration: u64,
}

impl Track {
    /// Duración de la pista expresada en la escala de tiempo del "movie".
    pub(super) fn movie_duration(&self, movie_ts: u32) -> u64 {
        (self.duration as f64 / self.timescale as f64 * movie_ts as f64).round() as u64
    }
}

#[derive(Default)]
pub(super) struct Trex {
    pub(super) dur: u32,
    pub(super) size: u32,
    pub(super) flags: u32,
}

/// Tipo de pista a leer. `Any` toma la primera (los MP4 de YouTube traen una sola).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Handler {
    Any,
    Audio,
}

pub(super) fn read_track(path: &Path, handler: Handler) -> Res<Track> {
    let file = File::open(path)?;
    let (moov, moofs) = scan_file(&file, file.metadata()?.len())?;
    let top = top_of(&moov);
    let trak = pick_trak(&moov, &top, handler)?;
    let mdia = find(&moov, &trak, b"mdia")?;
    let mdhd = find(&moov, &mdia, b"mdhd")?;
    let stbl = find(&moov, &find(&moov, &mdia, b"minf")?, b"stbl")?;
    let stsd = find(&moov, &stbl, b"stsd")?;
    let ts_off = mdhd.body + if u8_at(&moov, mdhd.body)? == 1 { 20 } else { 12 };
    let timescale = u32_at(&moov, ts_off)?;
    if timescale == 0 {
        return Err("MP4 corrupto: timescale 0".into());
    }
    let fourcc: [u8; 4] = rd(&moov, stsd.body + 12, 4)?.try_into().unwrap();

    // Valores por defecto de las muestras (mvex/trex)
    let mut trex = Trex::default();
    for c in children(&moov, top.body, top.end)? {
        if &c.typ == b"mvex" {
            let t = find(&moov, &c, b"trex")?;
            trex = Trex {
                dur: u32_at(&moov, t.body + 12)?,
                size: u32_at(&moov, t.body + 16)?,
                flags: u32_at(&moov, t.body + 20)?,
            };
        }
    }

    let mut track = Track {
        path: path.to_path_buf(),
        moov,
        trak,
        stsd,
        timescale,
        fourcc,
        samples: Vec::new(),
        chunks: Vec::new(),
        duration: 0,
    };
    if moofs.is_empty() {
        parse_stbl(&mut track, &stbl)?;
    } else {
        let mut dts = 0u64;
        for (start, buf) in &moofs {
            parse_moof(*start, buf, &trex, &mut track, &mut dts)?;
        }
    }
    if track.samples.is_empty() {
        return Err(format!("No hay muestras en {}", path.display()).into());
    }
    let file_len = file.metadata()?.len();
    if track.chunks.iter().any(|c| c.src.checked_add(c.len).is_none_or(|end| end > file_len)) {
        return Err("MP4 corrupto: un fragmento apunta fuera del archivo".into());
    }
    track.duration = track.samples.iter().map(|s| s.dur as u64).sum();
    Ok(track)
}

fn pick_trak(moov: &[u8], top: &BoxNode, handler: Handler) -> Res<BoxNode> {
    for trak in children(moov, top.body, top.end)?.into_iter().filter(|c| &c.typ == b"trak") {
        if handler == Handler::Any {
            return Ok(trak);
        }
        let hdlr = find(moov, &find(moov, &trak, b"mdia")?, b"hdlr")?;
        if rd(moov, hdlr.body + 8, 4)? == b"soun" {
            return Ok(trak);
        }
    }
    Err("El MP4 no tiene pista de audio".into())
}

/// Lee la lista de entradas de una caja de tabla (stts, stsc...) sin reservar memoria por
/// adelantado: el número de entradas viene del archivo y no es de fiar.
fn table(b: &[u8], node: &BoxNode, entry: usize, header: usize) -> Res<(usize, usize)> {
    let count = u32_at(b, node.body + header)? as usize;
    let start = node.body + header + 4;
    rd(b, start, count.checked_mul(entry).ok_or("MP4 corrupto: tabla enorme")?)?;
    Ok((count, start))
}

/// Pista de un MP4 normal: las tablas de muestras (stsz, stts, stsc, stco...) están en el moov.
fn parse_stbl(track: &mut Track, stbl: &BoxNode) -> Res<()> {
    let b = &track.moov;
    let opt = |typ: &[u8; 4]| children(b, stbl.body, stbl.end).map(|k| k.into_iter().find(|c| &c.typ == typ));

    let stsz = find(b, stbl, b"stsz")?;
    let fixed = u32_at(b, stsz.body + 4)?;
    let n = u32_at(b, stsz.body + 8)? as usize;
    if n == 0 || n > (MAX_SAMPLES_PER_RUN as usize) * 4 {
        return Err("MP4 corrupto: número de muestras absurdo".into());
    }
    let mut samples = Vec::with_capacity(n.min(1 << 20));
    for i in 0..n {
        let size = if fixed != 0 { fixed } else { u32_at(b, stsz.body + 12 + i * 4)? };
        samples.push(Sample { dur: 0, size, cto: 0, sync: true });
    }

    // Duraciones (stts), comprimidas en tramos
    let stts = find(b, stbl, b"stts")?;
    let (runs, at) = table(b, &stts, 8, 4)?;
    let mut i = 0usize;
    for r in 0..runs {
        let (count, delta) = (u32_at(b, at + r * 8)? as usize, u32_at(b, at + r * 8 + 4)?);
        for s in samples.iter_mut().skip(i).take(count) {
            s.dur = delta;
        }
        i = i.saturating_add(count);
    }

    // Desfase de composición (fotogramas B)
    if let Some(ctts) = opt(b"ctts")? {
        let (runs, at) = table(b, &ctts, 8, 4)?;
        let mut i = 0usize;
        for r in 0..runs {
            let (count, off) = (u32_at(b, at + r * 8)? as usize, i32_at(b, at + r * 8 + 4)?);
            for s in samples.iter_mut().skip(i).take(count) {
                s.cto = off;
            }
            i = i.saturating_add(count);
        }
    }

    // Fotogramas clave: si no hay stss, todos lo son
    if let Some(stss) = opt(b"stss")? {
        let (count, at) = table(b, &stss, 4, 4)?;
        samples.iter_mut().for_each(|s| s.sync = false);
        for k in 0..count {
            let num = u32_at(b, at + k * 4)? as usize;
            if let Some(s) = num.checked_sub(1).and_then(|i| samples.get_mut(i)) {
                s.sync = true;
            }
        }
    }

    // Reparto de muestras en chunks (stsc) y posición de cada chunk (stco / co64)
    let stsc = find(b, stbl, b"stsc")?;
    let (groups, gat) = table(b, &stsc, 12, 4)?;
    let (offsets, wide) = match opt(b"stco")? {
        Some(n) => (n, false),
        None => (find(b, stbl, b"co64")?, true),
    };
    let (nchunks, oat) = table(b, &offsets, if wide { 8 } else { 4 }, 4)?;
    let (mut idx, mut dts) = (0usize, 0u64);
    let mut chunks = Vec::with_capacity(nchunks.min(1 << 20));
    for c in 0..nchunks {
        // Último grupo cuyo primer chunk (base 1) es <= c + 1
        let mut per = 0usize;
        for g in 0..groups {
            if u32_at(b, gat + g * 12)? as usize <= c + 1 {
                per = u32_at(b, gat + g * 12 + 4)? as usize;
            }
        }
        let src = if wide { u64_at(b, oat + c * 8)? } else { u32_at(b, oat + c * 4)? as u64 };
        let part = samples.get(idx..idx.saturating_add(per)).ok_or("MP4 corrupto: tablas de muestras incoherentes")?;
        chunks.push(Chunk {
            src,
            len: part.iter().map(|s| s.size as u64).sum(),
            count: per as u32,
            time: dts as f64 / track.timescale as f64,
            rel: 0,
            out: 0,
        });
        dts += part.iter().map(|s| s.dur as u64).sum::<u64>();
        idx += per;
    }
    if idx != samples.len() {
        return Err("MP4 corrupto: las tablas de muestras no cuadran".into());
    }
    track.samples = samples;
    track.chunks = chunks;
    Ok(())
}

pub(super) fn parse_moof(moof_start: u64, buf: &[u8], trex: &Trex, track: &mut Track, dts: &mut u64) -> Res<()> {
    for traf in children(buf, 8, buf.len())? {
        if &traf.typ != b"traf" {
            continue;
        }
        let kids = children(buf, traf.body, traf.end)?;
        let tfhd = kids.iter().find(|k| &k.typ == b"tfhd").ok_or("traf sin tfhd")?;
        let tf = flags_at(buf, tfhd.body)?;
        let mut o = tfhd.body + 8;
        let mut base = moof_start;
        let (mut def_dur, mut def_size, mut def_flags) = (trex.dur, trex.size, trex.flags);
        if tf & 0x1 != 0 {
            base = u64_at(buf, o)?;
            o += 8;
        }
        if tf & 0x2 != 0 {
            o += 4;
        }
        if tf & 0x8 != 0 {
            def_dur = u32_at(buf, o)?;
            o += 4;
        }
        if tf & 0x10 != 0 {
            def_size = u32_at(buf, o)?;
            o += 4;
        }
        if tf & 0x20 != 0 {
            def_flags = u32_at(buf, o)?;
        }

        if let Some(tfdt) = kids.iter().find(|k| &k.typ == b"tfdt") {
            *dts = if u8_at(buf, tfdt.body)? == 1 { u64_at(buf, tfdt.body + 4)? } else { u32_at(buf, tfdt.body + 4)? as u64 };
        }

        let mut cursor = base;
        for trun in kids.iter().filter(|k| &k.typ == b"trun") {
            let version = u8_at(buf, trun.body)?;
            let f = flags_at(buf, trun.body)?;
            let count = u32_at(buf, trun.body + 4)?;
            let mut p = trun.body + 8;
            if f & 0x1 != 0 {
                cursor = (base as i64 + i32_at(buf, p)? as i64) as u64;
                p += 4;
            }
            if count > MAX_SAMPLES_PER_RUN {
                return Err("MP4 corrupto: demasiadas muestras en un fragmento".into());
            }
            let mut first_flags = None;
            if f & 0x4 != 0 {
                first_flags = Some(u32_at(buf, p)?);
                p += 4;
            }

            let mut chunk = Chunk {
                src: cursor,
                len: 0,
                count,
                time: *dts as f64 / track.timescale as f64,
                rel: 0,
                out: 0,
            };
            for i in 0..count {
                let (mut dur, mut size, mut cto) = (def_dur, def_size, 0i32);
                let mut flags = if i == 0 { first_flags.unwrap_or(def_flags) } else { def_flags };
                if f & 0x100 != 0 {
                    dur = u32_at(buf, p)?;
                    p += 4;
                }
                if f & 0x200 != 0 {
                    size = u32_at(buf, p)?;
                    p += 4;
                }
                if f & 0x400 != 0 {
                    flags = u32_at(buf, p)?;
                    p += 4;
                }
                if f & 0x800 != 0 {
                    cto = if version == 1 { i32_at(buf, p)? } else { u32_at(buf, p)? as i32 };
                    p += 4;
                }
                track.samples.push(Sample { dur, size, cto, sync: flags & 0x10000 == 0 });
                cursor += size as u64;
                chunk.len += size as u64;
                *dts += dur as u64;
            }
            track.chunks.push(chunk);
        }
    }
    Ok(())
}
