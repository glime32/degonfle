use crate::bits::BitWriter;
use crate::huffman::{codes, limited_lengths};
use crate::inflate::{fixed_lit_lengths, BlockStats, CL_ORDER, DIST_BASE, DIST_EXTRA, LEN_BASE, LEN_EXTRA};

const WINDOW: usize = 32 * 1024;
const WMASK: usize = WINDOW - 1;
const HASH_BITS: u32 = 15;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const BLOCK_SYMBOLS: usize = 16 * 1024;

/// Réglages par niveau, repris de zlib : (longueur de chaîne max, recherche
/// paresseuse tant que la correspondance fait moins de, longueur « assez
/// bonne » pour arrêter de chercher, longueur à partir de laquelle la
/// recherche paresseuse se contente d'un quart de la chaîne)
fn params(level: u32) -> (usize, usize, usize, usize) {
    match level {
        1 => (4, 0, 8, 4),
        2 => (8, 0, 16, 4),
        3 => (32, 0, 32, 4),
        4 => (16, 16, 16, 4),
        5 => (32, 16, 32, 8),
        6 => (128, 16, 128, 8),
        7 => (256, 32, 128, 8),
        8 => (1024, 128, 258, 32),
        _ => (4096, 258, 258, 32),
    }
}

#[derive(Clone, Copy)]
enum Sym {
    Lit(u8),
    Match { len: u16, dist: u16 },
}

fn len_code(len: usize) -> usize {
    // dernier code dont la base est <= len
    LEN_BASE.partition_point(|&b| b as usize <= len) - 1
}
fn dist_code(dist: usize) -> usize {
    DIST_BASE.partition_point(|&b| b as usize <= dist) - 1
}

/// Longueur commune de d[a..] et d[b..] (au plus max), 8 octets à la fois :
/// le premier octet différent est donné par les zéros de poids faible du XOR.
#[inline]
fn match_len(d: &[u8], a: usize, b: usize, max: usize) -> usize {
    let mut l = 0;
    while l + 8 <= max {
        let x = u64::from_le_bytes(d[a + l..a + l + 8].try_into().unwrap());
        let y = u64::from_le_bytes(d[b + l..b + l + 8].try_into().unwrap());
        let diff = x ^ y;
        if diff != 0 {
            return l + (diff.trailing_zeros() / 8) as usize;
        }
        l += 8;
    }
    while l < max && d[a + l] == d[b + l] {
        l += 1;
    }
    l
}

struct Matcher<'a> {
    data: &'a [u8],
    head: Vec<i32>,
    prev: Vec<i32>,
    max_chain: usize,
    nice: usize,
    good: usize,
}

impl<'a> Matcher<'a> {
    #[inline]
    fn hash(&self, i: usize) -> usize {
        let d = self.data;
        let h = (d[i] as u32) << 16 | (d[i + 1] as u32) << 8 | d[i + 2] as u32;
        (h.wrapping_mul(2654435761) >> (32 - HASH_BITS)) as usize
    }

    #[inline]
    fn insert(&mut self, i: usize) {
        if i + MIN_MATCH <= self.data.len() {
            let h = self.hash(i);
            self.prev[i & WMASK] = self.head[h];
            self.head[h] = i as i32;
        }
    }

    /// Meilleure correspondance pour la position i, parmi les positions déjà insérées.
    fn longest(&self, i: usize, at_least: usize) -> (usize, usize) {
        let d = self.data;
        let max = (d.len() - i).min(MAX_MATCH);
        if max < MIN_MATCH {
            return (0, 0);
        }
        let mut best = (at_least.max(MIN_MATCH - 1), 0);
        let mut cand = self.head[self.hash(i)];
        // déjà une bonne correspondance en main : on cherche moins longtemps
        let mut chain = if at_least >= self.good { self.max_chain / 4 } else { self.max_chain };
        while cand >= 0 && chain > 0 {
            let c = cand as usize;
            if i - c > WINDOW - 1 {
                break;
            }
            // test rapide sur l'octet qui ferait battre le record
            if d[c + best.0.min(max - 1)] == d[i + best.0.min(max - 1)] {
                let l = match_len(d, c, i, max);
                if l > best.0 {
                    best = (l, i - c);
                    if l >= self.nice || l == max {
                        break;
                    }
                }
            }
            let p = self.prev[c & WMASK];
            if p >= cand {
                break; // la case a été recyclée par une position plus récente
            }
            cand = p;
            chain -= 1;
        }
        if best.1 == 0 { (0, 0) } else { best }
    }
}

/// LZ77 avec recherche paresseuse : avant d'accepter une correspondance en i,
/// on regarde si i+1 en offre une plus longue.
fn lz77(data: &[u8], level: u32) -> Vec<Sym> {
    let (max_chain, lazy, nice, good) = params(level);
    let mut m = Matcher { data, head: vec![-1; 1 << HASH_BITS], prev: vec![-1; WINDOW], max_chain, nice, good };
    let mut syms = Vec::with_capacity(data.len() / 2);
    let mut i = 0;
    while i < data.len() {
        let (mut len, mut dist) = m.longest(i, 0);
        m.insert(i);
        if len >= MIN_MATCH && len < lazy && i + 1 < data.len() {
            let (l2, d2) = m.longest(i + 1, len);
            if l2 > len {
                syms.push(Sym::Lit(data[i]));
                i += 1;
                m.insert(i);
                len = l2;
                dist = d2;
            }
        }
        if len >= MIN_MATCH {
            syms.push(Sym::Match { len: len as u16, dist: dist as u16 });
            for k in i + 1..i + len {
                m.insert(k);
            }
            i += len;
        } else {
            syms.push(Sym::Lit(data[i]));
            i += 1;
        }
    }
    syms
}

struct Tree {
    lengths: Vec<u8>,
    codes: Vec<u16>,
}

impl Tree {
    fn new(lengths: Vec<u8>) -> Tree {
        let codes = codes(&lengths);
        Tree { lengths, codes }
    }
    #[inline]
    fn put(&self, w: &mut BitWriter, sym: usize) {
        w.put(self.codes[sym] as u32, self.lengths[sym] as u32);
    }
}

fn frequencies(syms: &[Sym]) -> (Vec<u32>, Vec<u32>) {
    let mut lit = vec![0u32; 286];
    let mut dist = vec![0u32; 30];
    for s in syms {
        match *s {
            Sym::Lit(b) => lit[b as usize] += 1,
            Sym::Match { len, dist: d } => {
                lit[257 + len_code(len as usize)] += 1;
                dist[dist_code(d as usize)] += 1;
            }
        }
    }
    lit[256] = 1;
    (lit, dist)
}

/// Longueurs des codes de longueurs, compressées par plages (symboles 16, 17, 18).
fn rle_lengths(lengths: &[u8]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lengths.len() {
        let v = lengths[i];
        let mut run = 1;
        while i + run < lengths.len() && lengths[i + run] == v {
            run += 1;
        }
        i += run;
        if v == 0 {
            while run >= 11 {
                let n = run.min(138);
                out.push((18, (n - 11) as u8));
                run -= n;
            }
            if run >= 3 {
                out.push((17, (run - 3) as u8));
                run = 0;
            }
        } else {
            out.push((v, 0));
            run -= 1;
            while run >= 3 {
                let n = run.min(6);
                out.push((16, (n - 3) as u8));
                run -= n;
            }
        }
        for _ in 0..run {
            out.push((v, 0));
        }
    }
    out
}

fn cl_extra_bits(sym: u8) -> u32 {
    match sym {
        16 => 2,
        17 => 3,
        18 => 7,
        _ => 0,
    }
}

/// Coût en bits du contenu d'un bloc avec ces arbres.
fn body_cost(syms: &[Sym], lit: &[u8], dist: &[u8]) -> u64 {
    let mut bits = lit[256] as u64;
    for s in syms {
        bits += match *s {
            Sym::Lit(b) => lit[b as usize] as u64,
            Sym::Match { len, dist: d } => {
                let lc = len_code(len as usize);
                let dc = dist_code(d as usize);
                (lit[257 + lc] + LEN_EXTRA[lc] + dist[dc] + DIST_EXTRA[dc]) as u64
            }
        };
    }
    bits
}

struct Dynamic {
    lit: Tree,
    dist: Tree,
    hlit: usize,
    hdist: usize,
    cl: Tree,
    rle: Vec<(u8, u8)>,
    hclen: usize,
}

fn build_dynamic(syms: &[Sym]) -> Dynamic {
    let (mut lf, mut df) = frequencies(syms);
    // comme zlib : au moins deux codes par arbre, certains décodeurs n'aiment pas un code unique
    for f in [&mut lf, &mut df] {
        let used = f.iter().filter(|&&x| x > 0).count();
        for _ in used..2 {
            let free = f.iter().position(|&x| x == 0).unwrap();
            f[free] = 1;
        }
    }
    let lit = limited_lengths(&lf, 15);
    let dist = limited_lengths(&df, 15);
    let hlit = 257.max(lit.iter().rposition(|&l| l > 0).unwrap() + 1);
    let hdist = 1.max(dist.iter().rposition(|&l| l > 0).map_or(0, |p| p + 1));
    let mut all = lit[..hlit].to_vec();
    all.extend_from_slice(&dist[..hdist]);
    let rle = rle_lengths(&all);
    let mut cf = vec![0u32; 19];
    for &(s, _) in &rle {
        cf[s as usize] += 1;
    }
    let cl = limited_lengths(&cf, 7);
    let hclen = 4.max(CL_ORDER.iter().rposition(|&i| cl[i] > 0).unwrap() + 1);
    Dynamic { lit: Tree::new(lit), dist: Tree::new(dist), hlit, hdist, cl: Tree::new(cl), rle, hclen }
}

impl Dynamic {
    fn header_cost(&self) -> u64 {
        let mut bits = 5 + 5 + 4 + 3 * self.hclen as u64;
        for &(s, _) in &self.rle {
            bits += self.cl.lengths[s as usize] as u64 + cl_extra_bits(s) as u64;
        }
        bits
    }

    fn write_header(&self, w: &mut BitWriter) {
        w.put((self.hlit - 257) as u32, 5);
        w.put((self.hdist - 1) as u32, 5);
        w.put((self.hclen - 4) as u32, 4);
        for &i in &CL_ORDER[..self.hclen] {
            w.put(self.cl.lengths[i] as u32, 3);
        }
        for &(s, extra) in &self.rle {
            self.cl.put(w, s as usize);
            w.put(extra as u32, cl_extra_bits(s));
        }
    }
}

fn write_body(w: &mut BitWriter, syms: &[Sym], lit: &Tree, dist: &Tree) {
    for s in syms {
        match *s {
            Sym::Lit(b) => lit.put(w, b as usize),
            Sym::Match { len, dist: d } => {
                let lc = len_code(len as usize);
                lit.put(w, 257 + lc);
                w.put((len - LEN_BASE[lc]) as u32, LEN_EXTRA[lc] as u32);
                let dc = dist_code(d as usize);
                dist.put(w, dc);
                w.put((d - DIST_BASE[dc]) as u32, DIST_EXTRA[dc] as u32);
            }
        }
    }
    lit.put(w, 256);
}

fn write_stored(w: &mut BitWriter, raw: &[u8], last: bool) {
    let chunks: Vec<&[u8]> = if raw.is_empty() { vec![raw] } else { raw.chunks(65535).collect() };
    for (k, c) in chunks.iter().enumerate() {
        w.put((last && k + 1 == chunks.len()) as u32, 1);
        w.put(0, 2);
        w.align();
        let len = c.len() as u32;
        w.put(len, 16);
        w.put(!len & 0xFFFF, 16);
        w.out.extend_from_slice(c);
    }
}

/// Compresse en DEFLATE brut. Pour chaque bloc on calcule le coût exact des
/// trois façons de l'écrire (non compressé, Huffman fixe, Huffman dynamique)
/// et on prend la moins chère.
pub fn deflate(data: &[u8], level: u32, stats: &mut BlockStats) -> Vec<u8> {
    let mut w = BitWriter::new();
    if level == 0 {
        write_stored(&mut w, data, true);
        stats.stored += 1;
        return w.finish();
    }
    let syms = lz77(data, level);
    let fixed_lit = Tree::new(fixed_lit_lengths());
    let fixed_dist = Tree::new(vec![5; 30]);
    let blocks: Vec<&[Sym]> = if syms.is_empty() { vec![&syms[..]] } else { syms.chunks(BLOCK_SYMBOLS).collect() };
    let mut pos = 0; // position dans les données d'origine
    for (k, b) in blocks.iter().enumerate() {
        let last = k + 1 == blocks.len();
        let raw_len: usize = b.iter().map(|s| match s {
            Sym::Lit(_) => 1,
            Sym::Match { len, .. } => *len as usize,
        }).sum();
        let raw = &data[pos..pos + raw_len];
        pos += raw_len;

        let dynamic = build_dynamic(b);
        let dyn_cost = 3 + dynamic.header_cost() + body_cost(b, &dynamic.lit.lengths, &dynamic.dist.lengths);
        let fix_cost = 3 + body_cost(b, &fixed_lit.lengths, &fixed_dist.lengths);
        let stored_cost = (raw_len as u64 + 5 * raw_len.div_ceil(65535).max(1) as u64) * 8 + 7;

        if stored_cost <= dyn_cost && stored_cost <= fix_cost {
            write_stored(&mut w, raw, last);
            stats.stored += 1;
        } else if fix_cost <= dyn_cost {
            w.put(last as u32, 1);
            w.put(1, 2);
            write_body(&mut w, b, &fixed_lit, &fixed_dist);
            stats.fixed += 1;
        } else {
            w.put(last as u32, 1);
            w.put(2, 2);
            dynamic.write_header(&mut w);
            write_body(&mut w, b, &dynamic.lit, &dynamic.dist);
            stats.dynamic += 1;
        }
    }
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inflate::inflate;

    fn round_trip(data: &[u8], level: u32) {
        let mut s = BlockStats::default();
        let z = deflate(data, level, &mut s);
        let mut out = Vec::new();
        let used = inflate(&z, &mut out, &mut BlockStats::default()).unwrap();
        assert_eq!(used, z.len());
        assert_eq!(out, data);
    }

    #[test]
    fn codes_tables() {
        assert_eq!(len_code(3), 0);
        assert_eq!(len_code(10), 7);
        assert_eq!(len_code(11), 8);
        assert_eq!(len_code(257), 27);
        assert_eq!(len_code(258), 28);
        assert_eq!(dist_code(1), 0);
        assert_eq!(dist_code(5), 4);
        assert_eq!(dist_code(32768), 29);
    }

    #[test]
    fn rle_round_trip() {
        let l = [0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 5, 5, 5, 5, 5, 5, 5, 3, 0, 0, 4];
        let mut back = Vec::new();
        for (s, e) in rle_lengths(&l) {
            match s {
                16 => {
                    let p = *back.last().unwrap();
                    for _ in 0..3 + e { back.push(p) }
                }
                17 => back.extend(std::iter::repeat_n(0, 3 + e as usize)),
                18 => back.extend(std::iter::repeat_n(0, 11 + e as usize)),
                v => back.push(v),
            }
        }
        assert_eq!(back, l);
    }

    #[test]
    fn various_inputs() {
        let mut x = 1u32;
        let random: Vec<u8> = (0..100_000).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; x as u8 }).collect();
        let text = "Le cheval de mon cousin ne mange du foin que le dimanche. ".repeat(3000);
        let runs: Vec<u8> = (0..200_000u32).map(|i| (i / 1000) as u8).collect();
        for level in [0, 1, 6, 9] {
            round_trip(b"", level);
            round_trip(b"a", level);
            round_trip(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", level);
            round_trip(&random, level);
            round_trip(text.as_bytes(), level);
            round_trip(&runs, level);
        }
    }
}
