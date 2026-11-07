// Le conteneur gzip (RFC 1952) : un en-tête de 10 octets, le flux DEFLATE,
// puis le CRC-32 et la taille des données d'origine.
use crate::deflate::deflate;
use crate::inflate::{inflate, BlockStats};

const FHCRC: u8 = 2;
const FEXTRA: u8 = 4;
const FNAME: u8 = 8;
const FCOMMENT: u8 = 16;

static CRC_TABLES: std::sync::OnceLock<[[u32; 256]; 8]> = std::sync::OnceLock::new();

/// CRC-32 « slicing-by-8 » : 8 tables pour avaler 8 octets par tour de boucle
/// au lieu d'un seul. C'était le goulot de la décompression.
pub fn crc32(data: &[u8]) -> u32 {
    let t = CRC_TABLES.get_or_init(|| {
        let mut t = [[0u32; 256]; 8];
        for n in 0..256 {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
            t[0][n] = c;
        }
        for n in 0..256 {
            for k in 1..8 {
                t[k][n] = (t[k - 1][n] >> 8) ^ t[0][(t[k - 1][n] & 0xFF) as usize];
            }
        }
        t
    });
    let mut c = !0u32;
    let mut chunks = data.chunks_exact(8);
    for b in &mut chunks {
        let lo = c ^ u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let hi = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        c = t[7][(lo & 0xFF) as usize]
            ^ t[6][((lo >> 8) & 0xFF) as usize]
            ^ t[5][((lo >> 16) & 0xFF) as usize]
            ^ t[4][(lo >> 24) as usize]
            ^ t[3][(hi & 0xFF) as usize]
            ^ t[2][((hi >> 8) & 0xFF) as usize]
            ^ t[1][((hi >> 16) & 0xFF) as usize]
            ^ t[0][(hi >> 24) as usize];
    }
    for &b in chunks.remainder() {
        c = t[0][((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

pub fn compress(data: &[u8], name: Option<&str>, mtime: u32, level: u32, stats: &mut BlockStats) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, if name.is_some() { FNAME } else { 0 }];
    out.extend(mtime.to_le_bytes());
    out.push(match level {
        9 => 2,
        1 => 4,
        _ => 0,
    });
    out.push(3); // Unix
    if let Some(n) = name {
        out.extend(n.bytes().filter(|&b| b != 0));
        out.push(0);
    }
    out.extend(deflate(data, level, stats));
    out.extend(crc32(data).to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out
}

/// Décompresse un fichier gzip, y compris plusieurs « membres » mis bout à bout
/// (ce que produit `cat a.gz b.gz`).
pub fn decompress(data: &[u8], stats: &mut BlockStats) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < data.len() {
        let d = &data[p..];
        if d.len() < 18 || d[0] != 0x1f || d[1] != 0x8b {
            return Err(if p == 0 { "ce n'est pas un fichier gzip".into() } else { "données en trop après la fin".into() });
        }
        if d[2] != 8 {
            return Err("méthode de compression inconnue (seul deflate existe)".into());
        }
        let flags = d[3];
        let mut h = 10;
        if flags & FEXTRA != 0 {
            let xlen = u16::from_le_bytes([d[h], d[h + 1]]) as usize;
            h += 2 + xlen;
        }
        for f in [FNAME, FCOMMENT] {
            if flags & f != 0 {
                h += d[h..].iter().position(|&b| b == 0).ok_or("en-tête tronqué")? + 1;
            }
        }
        if flags & FHCRC != 0 {
            h += 2;
        }
        if h > d.len() {
            return Err("en-tête tronqué".into());
        }
        let start = out.len();
        let used = inflate(&d[h..], &mut out, stats)?;
        let t = h + used;
        if t + 8 > d.len() {
            return Err("fin de fichier manquante (CRC et taille)".into());
        }
        let crc = u32::from_le_bytes(d[t..t + 4].try_into().unwrap());
        let size = u32::from_le_bytes(d[t + 4..t + 8].try_into().unwrap());
        if crc32(&out[start..]) != crc {
            return Err("CRC incorrect : fichier corrompu".into());
        }
        if (out.len() - start) as u32 != size {
            return Err("taille incorrecte : fichier corrompu".into());
        }
        p += t + 8;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc_check_value() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF43926);
        // les 8 tables doivent donner la même chose que l'algorithme octet par octet
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        for n in [0, 1, 7, 8, 9, 999, 1000] {
            let mut s = !0u32;
            for &b in &data[..n] {
                s ^= b as u32;
                for _ in 0..8 {
                    s = if s & 1 != 0 { 0xEDB88320 ^ (s >> 1) } else { s >> 1 };
                }
            }
            assert_eq!(super::crc32(&data[..n]), !s);
        }
    }
}
