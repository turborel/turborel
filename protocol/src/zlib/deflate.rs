//! \[EXTENSION\] Deflate, DEFLATE's encoder (RFC 1951, RULES.md D193):
//! LZ77 with hash chains (`lz77.rs`), then blocks of dynamic Huffman
//! codes (`huffman.rs`), each sent stored instead when that comes out
//! smaller, as data that does not compress does.

use super::tables::{
    CODE_LENGTH_ORDER, CODE_LENGTHS, DISTANCE_BASE, DISTANCE_EXTRA, DISTANCES, END_OF_BLOCK, FIRST_LENGTH, LENGTH_BASE, LENGTH_EXTRA,
    LITERALS, MAX_BITS, MAX_CODE_LENGTH_BITS, MAX_MATCH, MIN_CODE_LENGTH_SYMBOLS, MIN_DISTANCE_SYMBOLS, MIN_LITERAL_SYMBOLS,
    distance_index, length_index,
};

mod huffman;
mod lz77;

use lz77::Token;

/// The input bytes a block covers at most, so a block that does not
/// compress fits one stored block.
const BLOCK_SPAN: usize = 32_768;
/// A block's header bits: BFINAL, then BTYPE.
const BTYPE_BITS: u32 = 2;
/// BTYPE of a stored and a dynamic block.
const STORED: u32 = 0;
const DYNAMIC: u32 = 2;
/// The bits of a dynamic header's three counts and of each code length
/// code length.
const HLIT_BITS: u32 = 5;
const HDIST_BITS: u32 = 5;
const HCLEN_BITS: u32 = 4;
const CODE_LENGTH_BITS: u32 = 3;
/// A stored block's header beyond its first three bits: at most seven
/// bits to the byte's end, then LEN and NLEN.
const STORED_OVERHEAD_BITS: u64 = 7 + 32;

/// Bits written from the least significant end of each byte.
struct Writer {
    out: Vec<u8>,
    buffer: u64,
    count: u32,
}

impl Writer {
    fn put(&mut self, value: u32, bits: u32) {
        self.buffer |= u64::from(value) << self.count;
        self.count = self.count.saturating_add(bits);
        while self.count >= 8 {
            self.out.push(self.buffer.to_le_bytes().first().copied().unwrap_or(0));
            self.buffer >>= 8_u32;
            self.count = self.count.saturating_sub(8);
        }
    }

    /// The bits left written out, the last byte padded with zeros.
    fn align(&mut self) {
        if self.count > 0 {
            self.put(0, 8_u32.saturating_sub(self.count));
        }
    }
}

/// `data` as a DEFLATE stream.
pub(crate) fn deflate(data: &[u8]) -> Vec<u8> {
    let tokens = lz77::tokens(data);
    let mut w = Writer { out: Vec::with_capacity(data.len().checked_div(2).unwrap_or(0)), buffer: 0, count: 0 };
    let blocks = blocks(&tokens);
    if blocks.is_empty() {
        stored(&mut w, &[], true);
    }
    let (mut at, last) = (0_usize, blocks.len().saturating_sub(1));
    for (i, block) in blocks.iter().enumerate() {
        let span: usize = block.iter().map(|t| t.span()).sum();
        let raw = data.get(at..at.saturating_add(span)).unwrap_or_default();
        block_out(&mut w, block, raw, i == last);
        at = at.saturating_add(span);
    }
    w.align();
    w.out
}

/// `tokens` cut into blocks of at most `BLOCK_SPAN` input bytes.
fn blocks(tokens: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let (mut start, mut span) = (0_usize, 0_usize);
    for (i, t) in tokens.iter().enumerate() {
        if span.saturating_add(t.span()) > BLOCK_SPAN {
            out.push(tokens.get(start..i).unwrap_or_default());
            (start, span) = (i, 0);
        }
        span = span.saturating_add(t.span());
    }
    if start < tokens.len() {
        out.push(tokens.get(start..).unwrap_or_default());
    }
    out
}

/// One block, dynamic or stored, whichever is shorter.
fn block_out(w: &mut Writer, tokens: &[Token], raw: &[u8], last: bool) {
    let codes = Codes::of(tokens);
    let stored_bits = STORED_OVERHEAD_BITS.saturating_add(u64::try_from(raw.len()).unwrap_or(u64::MAX).saturating_mul(8));
    if codes.bits(tokens) >= stored_bits {
        stored(w, raw, last);
        return;
    }
    w.put(u32::from(last), 1);
    w.put(DYNAMIC, BTYPE_BITS);
    codes.header(w);
    for t in tokens {
        codes.token(w, *t);
    }
    codes.symbol(w, END_OF_BLOCK);
}

/// A stored block of `raw`, at most 65535 bytes.
fn stored(w: &mut Writer, raw: &[u8], last: bool) {
    w.put(u32::from(last), 1);
    w.put(STORED, BTYPE_BITS);
    w.align();
    let len = u16::try_from(raw.len()).unwrap_or(u16::MAX);
    w.out.extend_from_slice(&len.to_le_bytes());
    w.out.extend_from_slice(&(!len).to_le_bytes());
    w.out.extend_from_slice(raw);
}

/// A dynamic block's codes: lengths, and codes with their bits reversed.
struct Codes {
    literal_lengths: Vec<u8>,
    literal_reversed: Vec<u16>,
    distance_lengths: Vec<u8>,
    distance_reversed: Vec<u16>,
}

impl Codes {
    fn of(tokens: &[Token]) -> Codes {
        let mut literal_freqs = vec![0_u32; LITERALS];
        let mut distance_freqs = vec![0_u32; DISTANCES];
        let count = |freqs: &mut Vec<u32>, at: usize| {
            if let Some(f) = freqs.get_mut(at) {
                *f = f.saturating_add(1);
            }
        };
        count(&mut literal_freqs, usize::from(END_OF_BLOCK));
        for t in tokens {
            match *t {
                Token::Literal(b) => count(&mut literal_freqs, usize::from(b)),
                Token::Match { length, distance } => {
                    count(&mut literal_freqs, length_symbol(length));
                    count(&mut distance_freqs, distance_index(usize::from(distance)));
                }
            }
        }
        // Symbols 286, 287, 30 and 31 are never sent, so take no code.
        let literal_lengths = huffman::lengths(literal_freqs.get(..LITERALS.saturating_sub(2)).unwrap_or_default(), MAX_BITS);
        let distance_lengths = huffman::lengths(distance_freqs.get(..DISTANCES.saturating_sub(2)).unwrap_or_default(), MAX_BITS);
        let (literal_reversed, distance_reversed) = (huffman::codes(&literal_lengths), huffman::codes(&distance_lengths));
        Codes { literal_lengths, literal_reversed, distance_lengths, distance_reversed }
    }

    /// The bits the block's tokens and end of block take, its header
    /// included.
    fn bits(&self, tokens: &[Token]) -> u64 {
        let length_of = |lengths: &[u8], at: usize| u64::from(lengths.get(at).copied().unwrap_or(0));
        let body: u64 = tokens
            .iter()
            .map(|t| match *t {
                Token::Literal(b) => length_of(&self.literal_lengths, usize::from(b)),
                Token::Match { length, distance } => {
                    let (l, d) = (length_index(usize::from(length)), distance_index(usize::from(distance)));
                    length_of(&self.literal_lengths, length_symbol(length))
                        .saturating_add(u64::from(LENGTH_EXTRA.get(l).copied().unwrap_or(0)))
                        .saturating_add(length_of(&self.distance_lengths, d))
                        .saturating_add(u64::from(DISTANCE_EXTRA.get(d).copied().unwrap_or(0)))
                }
            })
            .sum();
        let mut probe = Writer { out: Vec::new(), buffer: 0, count: 0 };
        self.header(&mut probe);
        let header = u64::try_from(probe.out.len()).unwrap_or(0).saturating_mul(8).saturating_add(u64::from(probe.count));
        body.saturating_add(length_of(&self.literal_lengths, usize::from(END_OF_BLOCK))).saturating_add(header).saturating_add(3)
    }

    /// The dynamic header: the counts, the code length code, then the
    /// literal/length and distance code lengths coded by it (§3.2.7).
    fn header(&self, w: &mut Writer) {
        let hlit = used(&self.literal_lengths, MIN_LITERAL_SYMBOLS);
        let hdist = used(&self.distance_lengths, MIN_DISTANCE_SYMBOLS);
        let mut all: Vec<u8> = self.literal_lengths.iter().take(hlit).copied().collect();
        all.extend(self.distance_lengths.iter().take(hdist));
        let runs = huffman::runs(&all);
        let mut freqs = vec![0_u32; CODE_LENGTHS];
        for (symbol, _, _) in &runs {
            if let Some(f) = freqs.get_mut(usize::from(*symbol)) {
                *f = f.saturating_add(1);
            }
        }
        let lengths = huffman::lengths(&freqs, MAX_CODE_LENGTH_BITS);
        let codes = huffman::codes(&lengths);
        let ordered: Vec<u8> = CODE_LENGTH_ORDER.iter().map(|s| lengths.get(*s).copied().unwrap_or(0)).collect();
        let hclen = used(&ordered, MIN_CODE_LENGTH_SYMBOLS);
        w.put(count_bits(hlit, MIN_LITERAL_SYMBOLS), HLIT_BITS);
        w.put(count_bits(hdist, MIN_DISTANCE_SYMBOLS), HDIST_BITS);
        w.put(count_bits(hclen, MIN_CODE_LENGTH_SYMBOLS), HCLEN_BITS);
        for l in ordered.iter().take(hclen) {
            w.put(u32::from(*l), CODE_LENGTH_BITS);
        }
        for (symbol, extra, bits) in runs {
            let at = usize::from(symbol);
            put_code(w, codes.get(at), lengths.get(at));
            w.put(u32::from(extra), u32::from(bits));
        }
    }

    fn symbol(&self, w: &mut Writer, symbol: u16) {
        let at = usize::from(symbol);
        put_code(w, self.literal_reversed.get(at), self.literal_lengths.get(at));
    }

    fn token(&self, w: &mut Writer, t: Token) {
        let (length, distance) = match t {
            Token::Literal(b) => return self.symbol(w, u16::from(b)),
            Token::Match { length, distance } => (length, distance),
        };
        let l = length_index(usize::from(length));
        self.symbol(w, u16::try_from(length_symbol(length)).unwrap_or(END_OF_BLOCK));
        let length_base = LENGTH_BASE.get(l).copied().unwrap_or(0);
        w.put(u32::from(length.saturating_sub(length_base)), u32::from(LENGTH_EXTRA.get(l).copied().unwrap_or(0)));
        let d = distance_index(usize::from(distance));
        put_code(w, self.distance_reversed.get(d), self.distance_lengths.get(d));
        let distance_base = DISTANCE_BASE.get(d).copied().unwrap_or(0);
        w.put(u32::from(distance.saturating_sub(distance_base)), u32::from(DISTANCE_EXTRA.get(d).copied().unwrap_or(0)));
    }
}

/// A code, its bits reversed, of its length.
fn put_code(w: &mut Writer, code: Option<&u16>, length: Option<&u8>) {
    w.put(u32::from(code.copied().unwrap_or(0)), u32::from(length.copied().unwrap_or(0)));
}

/// The literal/length symbol of a match `length` long.
fn length_symbol(length: u16) -> usize {
    usize::from(FIRST_LENGTH).saturating_add(length_index(usize::from(length).min(MAX_MATCH)))
}

/// How many of `lengths` are sent: through the last nonzero, at least
/// `least`.
fn used(lengths: &[u8], least: usize) -> usize {
    lengths.iter().rposition(|l| *l != 0).map_or(0, |p| p.saturating_add(1)).max(least)
}

/// A header count as sent: less its least.
fn count_bits(n: usize, least: usize) -> u32 {
    u32::try_from(n.saturating_sub(least)).unwrap_or(0)
}
