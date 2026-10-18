//! Qué es de verdad un archivo de audio, según sus primeros bytes (las extensiones y los tipos
//! MIME de los servicios no son de fiar).

use std::io::Read;
use std::path::Path;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Container {
    Mp3,
    Mp4,
    Unknown,
}

pub fn sniff(path: &Path) -> Container {
    let mut head = [0u8; 12];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    match &head[..n] {
        h if h.starts_with(b"ID3") || (h.len() > 2 && h[0] == 0xff && h[1] & 0xe0 == 0xe0) => Container::Mp3,
        h if h.len() >= 8 && &h[4..8] == b"ftyp" => Container::Mp4,
        _ => Container::Unknown,
    }
}

#[cfg(test)]
mod tests {
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
