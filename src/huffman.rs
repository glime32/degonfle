// Codes de Huffman canoniques : seule la longueur de chaque code est transmise,
// les codes eux-mêmes s'en déduisent (dans l'ordre des longueurs, puis des symboles).

/// Longueurs de code optimales avec une longueur maximale imposée (15 bits
/// pour DEFLATE, 7 pour les codes de longueurs), par l'algorithme
/// « package-merge » de Larmore et Hirschberg.
pub fn limited_lengths(freqs: &[u32], max_len: u32) -> Vec<u8> {
    let mut lengths = vec![0u8; freqs.len()];
    let mut leaves: Vec<(u64, usize)> = freqs.iter().enumerate().filter(|&(_, &f)| f > 0).map(|(s, &f)| (f as u64, s)).collect();
    match leaves.len() {
        0 => return lengths,
        1 => {
            lengths[leaves[0].1] = 1;
            return lengths;
        }
        _ => {}
    }
    leaves.sort();

    // un élément est soit une feuille, soit un paquet de deux éléments du niveau précédent
    enum Node {
        Leaf(usize),
        Pair(usize, usize),
    }
    let mut nodes: Vec<Node> = leaves.iter().map(|&(_, s)| Node::Leaf(s)).collect();
    let leaf_items: Vec<(u64, usize)> = leaves.iter().enumerate().map(|(i, &(w, _))| (w, i)).collect();

    let mut list = leaf_items.clone();
    for _ in 1..max_len {
        let mut packages = Vec::with_capacity(list.len() / 2);
        for [a, b] in list.as_chunks::<2>().0 {
            nodes.push(Node::Pair(a.1, b.1));
            packages.push((a.0 + b.0, nodes.len() - 1));
        }
        // fusion de deux listes triées
        let mut merged = Vec::with_capacity(leaf_items.len() + packages.len());
        let (mut i, mut j) = (0, 0);
        while i < leaf_items.len() || j < packages.len() {
            if j == packages.len() || (i < leaf_items.len() && leaf_items[i].0 <= packages[j].0) {
                merged.push(leaf_items[i]);
                i += 1;
            } else {
                merged.push(packages[j]);
                j += 1;
            }
        }
        list = merged;
    }
    // les 2n-2 premiers éléments : chaque apparition d'une feuille ajoute 1 à sa longueur
    let mut stack: Vec<usize> = list[..2 * leaves.len() - 2].iter().map(|&(_, n)| n).collect();
    while let Some(n) = stack.pop() {
        match nodes[n] {
            Node::Leaf(s) => lengths[s] += 1,
            Node::Pair(a, b) => {
                stack.push(a);
                stack.push(b);
            }
        }
    }
    lengths
}

/// Codes canoniques, déjà inversés bit à bit (DEFLATE écrit les codes de
/// Huffman en commençant par le bit de poids fort, dans un flux poids faible d'abord).
pub fn codes(lengths: &[u8]) -> Vec<u16> {
    let mut count = [0u16; 16];
    for &l in lengths {
        count[l as usize] += 1;
    }
    count[0] = 0;
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for bits in 1..16 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let c = next[l as usize];
            next[l as usize] += 1;
            c.reverse_bits() >> (16 - l)
        })
        .collect()
}

/// Décodage par table : on lit `bits` bits d'un coup (10 au plus, pour que la
/// table tienne dans le cache) et la table donne le symbole et sa longueur.
/// Les rares codes plus longs passent par un décodage canonique bit à bit.
pub struct Decoder {
    pub bits: u32,
    table: Vec<u32>, // symbole << 4 | longueur ; 0 = code invalide ; LONG = code de plus de `bits` bits
    count: [u16; 16],
    symbols: Vec<u16>, // symboles triés par longueur de code puis par valeur
}

const PRIMARY: u32 = 10;
const LONG: u32 = 1;

impl Decoder {
    pub fn new(lengths: &[u8]) -> Result<Decoder, &'static str> {
        let max = *lengths.iter().max().unwrap_or(&0) as u32;
        let mut count = [0u16; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        // le code ne doit pas être « trop plein » (inégalité de Kraft)
        let mut left: i64 = 1;
        for &n in &count[1..] {
            left = left * 2 - n as i64;
            if left < 0 {
                return Err("code de Huffman sur-souscrit");
            }
        }
        let mut symbols: Vec<u16> = (0..lengths.len() as u16).filter(|&s| lengths[s as usize] > 0).collect();
        symbols.sort_by_key(|&s| lengths[s as usize]);

        let bits = max.clamp(1, PRIMARY);
        let mut table = vec![0u32; 1 << bits];
        let codes = codes(lengths);
        for (sym, &l) in lengths.iter().enumerate() {
            let l = l as u32;
            if l == 0 {
                continue;
            }
            if l <= bits {
                let mut i = codes[sym] as usize;
                while i < table.len() {
                    table[i] = (sym as u32) << 4 | l;
                    i += 1 << l;
                }
            } else {
                // les `bits` premiers bits lus de ce code mènent au chemin lent
                table[(codes[sym] as usize) & ((1 << bits) - 1)] = LONG;
            }
        }
        Ok(Decoder { bits, table, count, symbols })
    }

    #[inline]
    pub fn decode(&self, r: &mut crate::bits::BitReader) -> Result<u16, &'static str> {
        let e = self.table[r.peek(self.bits) as usize];
        if e > LONG {
            r.consume(e & 15);
            return Ok((e >> 4) as u16);
        }
        if e == 0 {
            return Err("code de Huffman invalide");
        }
        self.decode_slow(r)
    }

    fn decode_slow(&self, r: &mut crate::bits::BitReader) -> Result<u16, &'static str> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= r.bits(1) as i32;
            let n = self.count[len] as i32;
            if code - n < first {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += n;
            first = (first + n) << 1;
            code <<= 1;
        }
        Err("code de Huffman invalide")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kraft(lengths: &[u8]) -> f64 {
        lengths.iter().filter(|&&l| l > 0).map(|&l| 0.5f64.powi(l as i32)).sum()
    }

    #[test]
    fn respects_limit_and_kraft() {
        // fréquences de Fibonacci : sans limite, l'arbre serait un peigne très profond
        let mut f = vec![1u32, 1];
        while f.len() < 30 {
            let n = f.len();
            f.push(f[n - 1] + f[n - 2]);
        }
        let l = limited_lengths(&f, 15);
        assert!(l.iter().all(|&x| x <= 15 && x > 0));
        assert!((kraft(&l) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn matches_plain_huffman_when_unconstrained() {
        let f = [5u32, 9, 12, 13, 16, 45];
        let l = limited_lengths(&f, 15);
        let cost: u32 = f.iter().zip(&l).map(|(&a, &b)| a * b as u32).sum();
        assert_eq!(cost, 224); // coût de l'arbre de Huffman classique pour cet exemple
    }

    #[test]
    fn canonical_example_from_rfc() {
        // RFC 1951, 3.2.2 : longueurs (3,3,3,3,3,2,4,4) -> 010 011 100 101 110 00 1110 1111
        let c = codes(&[3, 3, 3, 3, 3, 2, 4, 4]);
        let expected = [0b010u16, 0b011, 0b100, 0b101, 0b110, 0b00, 0b1110, 0b1111];
        let lens = [3, 3, 3, 3, 3, 2, 4, 4];
        for i in 0..8 {
            assert_eq!(c[i], expected[i].reverse_bits() >> (16 - lens[i]));
        }
    }
}
