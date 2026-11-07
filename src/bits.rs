// DEFLATE range les bits en commençant par le poids faible de chaque octet.

pub struct BitWriter {
    pub out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        BitWriter { out: Vec::new(), acc: 0, n: 0 }
    }

    #[inline]
    pub fn put(&mut self, bits: u32, count: u32) {
        debug_assert!(count <= 32 && (count == 32 || bits >> count == 0));
        self.acc |= (bits as u64) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    pub fn align(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc = 0;
            self.n = 0;
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

pub struct BitReader<'a> {
    data: &'a [u8],
    pub pos: usize, // prochain octet à charger
    buf: u64,
    n: u32,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, buf: 0, n: 0 }
    }

    #[inline]
    fn refill(&mut self) {
        while self.n <= 56 {
            let b = if self.pos < self.data.len() { self.data[self.pos] } else { 0 };
            self.pos += 1; // on peut dépasser la fin : vérifié dans overrun()
            self.buf |= (b as u64) << self.n;
            self.n += 8;
        }
    }

    #[inline]
    pub fn peek(&mut self, count: u32) -> u32 {
        if self.n < count {
            self.refill();
        }
        (self.buf & ((1u64 << count) - 1)) as u32
    }

    #[inline]
    pub fn consume(&mut self, count: u32) {
        self.buf >>= count;
        self.n -= count;
    }

    #[inline]
    pub fn bits(&mut self, count: u32) -> u32 {
        if count == 0 {
            return 0;
        }
        let v = self.peek(count);
        self.consume(count);
        v
    }

    /// Jette les bits restants de l'octet en cours.
    pub fn align(&mut self) {
        let r = self.n % 8;
        self.consume(r);
    }

    /// Position (en octets) du prochain bit non lu.
    pub fn byte_pos(&self) -> usize {
        self.pos - (self.n / 8) as usize
    }

    pub fn overrun(&self) -> bool {
        self.pos * 8 - self.n as usize > self.data.len() * 8
    }
}
