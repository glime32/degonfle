use crate::bits::BitReader;
use crate::huffman::Decoder;

pub const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
pub const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
pub const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
pub const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
/// ordre dans lequel sont transmises les longueurs des codes de longueurs
pub const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

pub fn fixed_lit_lengths() -> Vec<u8> {
    (0..288).map(|s| match s {
        0..=143 => 8,
        144..=255 => 9,
        256..=279 => 7,
        _ => 8,
    }).collect()
}

#[derive(Default, Debug)]
pub struct BlockStats {
    pub stored: usize,
    pub fixed: usize,
    pub dynamic: usize,
}

/// Décompresse un flux DEFLATE brut. Renvoie aussi le nombre d'octets lus.
pub fn inflate(data: &[u8], out: &mut Vec<u8>, stats: &mut BlockStats) -> Result<usize, String> {
    let mut r = BitReader::new(data);
    loop {
        let last = r.bits(1) == 1;
        match r.bits(2) {
            0 => {
                r.align();
                let len = r.bits(16) as usize;
                let nlen = r.bits(16) as usize;
                if len != !nlen & 0xFFFF {
                    return Err("bloc non compressé : LEN et NLEN incohérents".into());
                }
                let start = r.byte_pos();
                if start + len > data.len() {
                    return Err("données tronquées".into());
                }
                out.extend_from_slice(&data[start..start + len]);
                r = BitReader::new(data);
                r.pos = start + len;
                stats.stored += 1;
            }
            1 => {
                let lit = Decoder::new(&fixed_lit_lengths())?;
                let dist = Decoder::new(&[5u8; 30])?;
                block(&mut r, out, &lit, &dist)?;
                stats.fixed += 1;
            }
            2 => {
                let (lit, dist) = read_dynamic_header(&mut r)?;
                block(&mut r, out, &lit, &dist)?;
                stats.dynamic += 1;
            }
            _ => return Err("type de bloc 3 : réservé, fichier corrompu".into()),
        }
        if r.overrun() {
            return Err("données tronquées".into());
        }
        if last {
            r.align();
            return Ok(r.byte_pos());
        }
    }
}

fn read_dynamic_header(r: &mut BitReader) -> Result<(Decoder, Decoder), String> {
    let hlit = r.bits(5) as usize + 257;
    let hdist = r.bits(5) as usize + 1;
    let hclen = r.bits(4) as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err("en-tête de bloc dynamique invalide".into());
    }
    let mut cl_lengths = [0u8; 19];
    for &i in &CL_ORDER[..hclen] {
        cl_lengths[i] = r.bits(3) as u8;
    }
    let cl = Decoder::new(&cl_lengths)?;
    let mut lengths = Vec::with_capacity(hlit + hdist);
    while lengths.len() < hlit + hdist {
        let sym = cl.decode(r)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let prev = *lengths.last().ok_or("répétition sans longueur précédente")?;
                for _ in 0..3 + r.bits(2) {
                    lengths.push(prev);
                }
            }
            17 => lengths.extend(std::iter::repeat_n(0, 3 + r.bits(3) as usize)),
            _ => lengths.extend(std::iter::repeat_n(0, 11 + r.bits(7) as usize)),
        }
    }
    if lengths.len() > hlit + hdist {
        return Err("répétition qui déborde".into());
    }
    if lengths[256] == 0 {
        return Err("pas de code pour la fin de bloc".into());
    }
    Ok((Decoder::new(&lengths[..hlit])?, Decoder::new(&lengths[hlit..])?))
}

fn block(r: &mut BitReader, out: &mut Vec<u8>, lit: &Decoder, dist: &Decoder) -> Result<(), String> {
    let mut until_check = 0u32;
    loop {
        let sym = lit.decode(r)? as usize;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let i = sym - 257;
            if i >= 29 {
                return Err("symbole de longueur invalide".into());
            }
            let len = LEN_BASE[i] as usize + r.bits(LEN_EXTRA[i] as u32) as usize;
            let d = dist.decode(r)? as usize;
            if d >= 30 {
                return Err("symbole de distance invalide".into());
            }
            let distance = DIST_BASE[d] as usize + r.bits(DIST_EXTRA[d] as u32) as usize;
            if distance > out.len() {
                return Err(format!("distance {distance} avant le début des données"));
            }
            let start = out.len() - distance;
            if distance >= len {
                out.extend_from_within(start..start + len);
            } else {
                // copie qui se recouvre : « abcabcabc... » à partir de « abc »
                for k in 0..len {
                    let b = out[start + k];
                    out.push(b);
                }
            }
        }
        // au-delà de la fin, le lecteur renvoie des zéros : on vérifie de temps en temps
        until_check += 1;
        if until_check == 4096 {
            until_check = 0;
            if r.overrun() {
                return Err("données tronquées".into());
            }
        }
    }
}
