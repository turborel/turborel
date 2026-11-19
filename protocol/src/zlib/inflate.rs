//! \[EXTENSION\] Inflate, DEFLATE's decoder (RFC 1951 §3.2, RULES.md
//! D193): stored, fixed and dynamic Huffman blocks, read a bit at a time
//! from the least significant end of each byte.
//!
//! Output is bounded. A stream that would inflate past the limit its
//! caller gives is refused as soon as it passes it, so a small stream
//! claiming gigabytes holds no more memory than the limit.

use turborel::error::Result;

use super::tables::{
    CODE_LENGTH_ORDER, CODE_LENGTHS, DISTANCE_BASE, DISTANCE_EXTRA, DISTANCES, END_OF_BLOCK, FIRST_LENGTH, FIXED_DISTANCE_LENGTH,
    LENGTH_BASE, LENGTH_EXTRA, MAX_DISTANCE_SYMBOLS, MAX_LITERAL_SYMBOLS, MIN_CODE_LENGTH_SYMBOLS, MIN_DISTANCE_SYMBOLS,
    MIN_LITERAL_SYMBOLS, REPEAT_PREVIOUS, REPEAT_ZERO, fixed_literal_lengths,
};
use super::{malformed, too_large};

mod huffman;

use huffman::Huffman;

/// The block types of a block's header (§3.2.3).
const STORED: u32 = 0;
const FIXED: u32 = 1;
const DYNAMIC: u32 = 2;
/// The bits of a dynamic header's counts: HLIT 5, HDIST 5, HCLEN 4, and
/// of each code length code length, 3.
const HLIT_BITS: u32 = 5;
const HDIST_BITS: u32 = 5;
const HCLEN_BITS: u32 = 4;
const CODE_LENGTH_BITS: u32 = 3;
/// A repeat's count bits and least count: 16 takes 2 bits from 3, 17
/// takes 3 from 3, 18 takes 7 from 11.
const REPEAT_PREVIOUS_BITS: (u32, usize) = (2, 3);
const REPEAT_ZERO_BITS: (u32, usize) = (3, 3);
const REPEAT_ZERO_LONG_BITS: (u32, usize) = (7, 11);

/// The bits of a stream, read from the least significant end of each byte.
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    buffer: u32,
    count: u32,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Bits<'a> {
        Bits { data, at: 0, buffer: 0, count: 0 }
    }

    /// The next `n` bits, n at most 16, the first read the least
    /// significant.
    pub(crate) fn bits(&mut self, n: u32) -> Result<u32> {
        while self.count < n {
            let byte = self.data.get(self.at).ok_or_else(|| malformed("the stream ends inside a block"))?;
            self.buffer |= u32::from(*byte) << self.count;
            self.at = self.at.saturating_add(1);
            self.count = self.count.saturating_add(8);
        }
        let value = self.buffer & (1_u32 << n).wrapping_sub(1);
        self.buffer = self.buffer.checked_shr(n).unwrap_or(0);
        self.count = self.count.saturating_sub(n);
        Ok(value)
    }

    /// The rest of the current byte dropped, as a stored block begins.
    fn align(&mut self) {
        self.buffer = 0;
        self.count = 0;
    }

    /// `n` whole bytes, after `align`.
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(|| malformed("a stored block's length"))?;
        let taken = self.data.get(self.at..end).ok_or_else(|| malformed("the stream ends inside a stored block"))?;
        self.at = end;
        Ok(taken)
    }

    /// The bytes after the last whole byte read.
    pub(crate) fn rest(&self) -> &'a [u8] {
        self.data.get(self.at..).unwrap_or_default()
    }
}

/// The output so far, at most `limit` bytes.
struct Output {
    out: Vec<u8>,
    limit: usize,
}

impl Output {
    fn push(&mut self, byte: u8) -> Result<()> {
        if self.out.len() >= self.limit {
            return Err(too_large(self.limit));
        }
        self.out.push(byte);
        Ok(())
    }

    /// `length` bytes copied from `distance` back, which may overlap
    /// what they write.
    fn copy(&mut self, distance: usize, length: usize) -> Result<()> {
        let from = self.out.len().checked_sub(distance).ok_or_else(|| malformed("a distance reaches before the stream's start"))?;
        for i in 0..length {
            let byte = self.out.get(from.saturating_add(i)).copied().unwrap_or(0);
            self.push(byte)?;
        }
        Ok(())
    }
}

/// The DEFLATE stream at the start of `bits` inflated, at most `limit`
/// bytes; `bits` is left after its last block.
pub(crate) fn inflate(bits: &mut Bits<'_>, limit: usize) -> Result<Vec<u8>> {
    let mut output = Output { out: Vec::new(), limit };
    loop {
        let last = bits.bits(1)? == 1;
        match bits.bits(2)? {
            STORED => stored(bits, &mut output)?,
            FIXED => {
                let (literals, distances) = fixed()?;
                codes(bits, &mut output, &literals, &distances)?;
            }
            DYNAMIC => {
                let (literals, distances) = dynamic(bits)?;
                codes(bits, &mut output, &literals, &distances)?;
            }
            _ => return Err(malformed("a block of the reserved type 3")),
        }
        if last {
            return Ok(output.out);
        }
    }
}

/// A stored block: LEN, its complement NLEN, then LEN bytes (§3.2.4).
fn stored(bits: &mut Bits<'_>, output: &mut Output) -> Result<()> {
    bits.align();
    let header = bits.bytes(4)?;
    let len = u16::from_le_bytes([header.first().copied().unwrap_or(0), header.get(1).copied().unwrap_or(0)]);
    let nlen = u16::from_le_bytes([header.get(2).copied().unwrap_or(0), header.get(3).copied().unwrap_or(0)]);
    if len != !nlen {
        return Err(malformed("a stored block's length and its complement differ"));
    }
    for byte in bits.bytes(usize::from(len))? {
        output.push(*byte)?;
    }
    Ok(())
}

/// The fixed codes (§3.2.6).
fn fixed() -> Result<(Huffman, Huffman)> {
    let literals = Huffman::new(&fixed_literal_lengths())?;
    let distances = Huffman::new(&[FIXED_DISTANCE_LENGTH; DISTANCES])?;
    Ok((literals, distances))
}

/// A dynamic block's codes, from its header (§3.2.7).
fn dynamic(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman)> {
    let nlen = usize::try_from(bits.bits(HLIT_BITS)?).unwrap_or(0).saturating_add(MIN_LITERAL_SYMBOLS);
    let ndist = usize::try_from(bits.bits(HDIST_BITS)?).unwrap_or(0).saturating_add(MIN_DISTANCE_SYMBOLS);
    let ncode = usize::try_from(bits.bits(HCLEN_BITS)?).unwrap_or(0).saturating_add(MIN_CODE_LENGTH_SYMBOLS);
    if nlen > MAX_LITERAL_SYMBOLS || ndist > MAX_DISTANCE_SYMBOLS {
        return Err(malformed("a dynamic block counts too many codes"));
    }
    let mut code_lengths = [0_u8; CODE_LENGTHS];
    for symbol in CODE_LENGTH_ORDER.iter().take(ncode) {
        let length = u8::try_from(bits.bits(CODE_LENGTH_BITS)?).unwrap_or(0);
        if let Some(slot) = code_lengths.get_mut(*symbol) {
            *slot = length;
        }
    }
    let lengths_code = Huffman::complete(&code_lengths)?;
    let lengths = code_lengths_of(bits, &lengths_code, nlen.saturating_add(ndist))?;
    let (literal_lengths, distance_lengths) = lengths.split_at(nlen);
    if literal_lengths.get(usize::from(END_OF_BLOCK)).copied().unwrap_or(0) == 0 {
        return Err(malformed("a dynamic block's code has no end of block"));
    }
    Ok((Huffman::new(literal_lengths)?, Huffman::new(distance_lengths)?))
}

/// The `n` code lengths of a dynamic block, run-length coded.
fn code_lengths_of(bits: &mut Bits<'_>, code: &Huffman, n: usize) -> Result<Vec<u8>> {
    let mut lengths: Vec<u8> = Vec::with_capacity(n);
    while lengths.len() < n {
        let symbol = code.decode(bits)?;
        let (repeated, (extra, least)) = match symbol {
            0..=15 => {
                lengths.push(u8::try_from(symbol).unwrap_or(0));
                continue;
            }
            REPEAT_PREVIOUS => (*lengths.last().ok_or_else(|| malformed("a repeat with no length before it"))?, REPEAT_PREVIOUS_BITS),
            REPEAT_ZERO => (0, REPEAT_ZERO_BITS),
            _ => (0, REPEAT_ZERO_LONG_BITS),
        };
        let times = usize::try_from(bits.bits(extra)?).unwrap_or(0).saturating_add(least);
        if lengths.len().saturating_add(times) > n {
            return Err(malformed("a repeat runs past the code lengths counted"));
        }
        lengths.extend(std::iter::repeat_n(repeated, times));
    }
    Ok(lengths)
}

/// A Huffman block's symbols, to its end of block (§3.2.5).
fn codes(bits: &mut Bits<'_>, output: &mut Output, literals: &Huffman, distances: &Huffman) -> Result<()> {
    loop {
        let symbol = literals.decode(bits)?;
        if symbol < END_OF_BLOCK {
            output.push(u8::try_from(symbol).unwrap_or(0))?;
            continue;
        }
        if symbol == END_OF_BLOCK {
            return Ok(());
        }
        let l = usize::from(symbol.saturating_sub(FIRST_LENGTH));
        let length = extra(bits, LENGTH_BASE.get(l), LENGTH_EXTRA.get(l))?;
        let d = usize::from(distances.decode(bits)?);
        let distance = extra(bits, DISTANCE_BASE.get(d), DISTANCE_EXTRA.get(d))?;
        output.copy(distance, length)?;
    }
}

/// A base and its extra bits read, of a symbol in range.
fn extra(bits: &mut Bits<'_>, base: Option<&u16>, extra: Option<&u8>) -> Result<usize> {
    let (Some(base), Some(extra)) = (base, extra) else { return Err(malformed("a length or distance symbol out of range")) };
    let more = bits.bits(u32::from(*extra))?;
    Ok(usize::from(*base).saturating_add(usize::try_from(more).unwrap_or(0)))
}

/// Whether `lengths` make a code `Huffman` takes, as a code length code
/// if `complete`: for the tests.
#[cfg(test)]
pub(crate) fn huffman_check(lengths: &[u8], complete: bool) -> Result<()> {
    if complete { Huffman::complete(lengths).map(|_| ()) } else { Huffman::new(lengths).map(|_| ()) }
}
