//! \[DESIGN\] A posting block of the sparse inverted index: up to a block
//! size of (row, weight) postings of one dimension, rows ascending, kept
//! as one value of the index's tree. Its layout, little-endian:
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 1 | format, 1 |
//! | 1 | 1 | weight quantization: 0 FLOAT32, 1 FLOAT16, 2 UINT8 |
//! | 2 | 2 | postings |
//! | 4 | 4 | greatest weight, a bound of the weights inserted |
//! | 8 | 4 | least weight, likewise |
//! | 12 | 4 | error: the most a stored weight is from the weight inserted |
//! | 16 | 1 | bits of each row delta |
//! | 17 | 1-10 | first row, LEB128 |
//! | . | 1-10 | last row less the first, LEB128 |
//! | . | . | row deltas, bit-packed, low bits first |
//! | . | . | weights: 4, 2 or 1 bytes each |
//!
//! Row ids are delta-coded and bit-packed at the width of the greatest
//! delta, as frame-of-reference coding packs integers (Goldstein,
//! Ramakrishnan and Shaft, "Compressing Relations and Indexes", ICDE 1998).
//! A delta of 0 is a second posting of one row, another version's.
//!
//! FLOAT16 weights are scaled by the block's greatest magnitude first, so
//! none overflows a half; UINT8 weights are 255 even steps from the least
//! weight to the greatest. The greatest and least weights are kept exactly:
//! they are the block-max bounds of Block-Max WAND (`wand.rs`), which must
//! bound the weights inserted, not those stored. The error is kept so a
//! score from stored weights can be widened into a bound of the true one.
//! Each re-encoding adds what it measures to the error inherited, so the
//! error bounds the distance from every weight ever inserted.

use super::half::{from_half, to_half};
use super::vector::narrow;
use crate::error::{Code, Result, SqlError};

/// The block format.
const BLOCK_FORMAT: u8 = 1;
/// The bytes of the fixed header, before the first row.
pub const HEADER_BYTES: usize = 17;
/// UINT8 steps between the least weight and the greatest.
const BYTE_STEPS: f64 = 255.0;
/// The bits of a byte.
const BYTE_BITS: u32 = 8;
/// The payload bits of a LEB128 byte.
const LEB_BITS: u32 = 7;
/// A LEB128 byte's continuation flag.
const LEB_MORE: u8 = 0x80;
/// The bytes of a FLOAT32 weight.
const FLOAT32_BYTES: usize = 4;
/// The bytes of a FLOAT16 weight.
const FLOAT16_BYTES: usize = 2;
/// The bits of a row id.
const ROW_BITS: u32 = 64;

/// \[EXTENSION\] How a block stores its weights (SPARSE.QUANTIZATION).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantization {
    /// Single precision, as inserted.
    Float32,
    /// Half precision, scaled by the block's greatest magnitude.
    Float16,
    /// A byte, of 255 steps from the block's least weight to its greatest.
    Uint8,
}

/// The option values naming each quantization, in code order.
pub const QUANTIZATIONS: [&str; 3] = ["FLOAT32", "FLOAT16", "UINT8"];

impl Quantization {
    /// The quantization `name` names, whatever its case.
    pub fn named(name: &str) -> Option<Quantization> {
        [Quantization::Float32, Quantization::Float16, Quantization::Uint8].into_iter().find(|q| q.name().eq_ignore_ascii_case(name))
    }

    /// Its option value.
    pub fn name(self) -> &'static str {
        let [float32, float16, uint8] = QUANTIZATIONS;
        match self {
            Quantization::Float32 => float32,
            Quantization::Float16 => float16,
            Quantization::Uint8 => uint8,
        }
    }

    /// Its code in a block.
    pub fn code(self) -> u8 {
        match self {
            Quantization::Float32 => 0,
            Quantization::Float16 => 1,
            Quantization::Uint8 => 2,
        }
    }

    fn of_code(code: u8) -> Result<Quantization> {
        [Quantization::Float32, Quantization::Float16, Quantization::Uint8]
            .into_iter()
            .find(|q| q.code() == code)
            .ok_or_else(|| corrupt("its weight quantization is unknown"))
    }

    /// The bytes of each weight.
    fn width(self) -> usize {
        match self {
            Quantization::Float32 => FLOAT32_BYTES,
            Quantization::Float16 => FLOAT16_BYTES,
            Quantization::Uint8 => 1,
        }
    }
}

pub(super) fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a sparse index posting block is malformed: {why}"))
}

/// What a block's header says, read without its postings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Header {
    /// Its postings.
    pub count: usize,
    /// Its first row.
    pub first: u64,
    /// Its last row.
    pub last: u64,
    /// At least every weight inserted.
    pub max: f32,
    /// At most every weight inserted.
    pub min: f32,
    /// At least how far a stored weight is from its weight inserted.
    pub error: f32,
    /// How its weights are stored.
    pub quantization: Quantization,
    /// The bits of each row delta.
    bits: u32,
    /// Where its row deltas start.
    deltas: usize,
}

impl Header {
    /// The header of the block `bytes`, and where its row deltas start.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not a block of this format.
    pub fn read(bytes: &[u8]) -> Result<Header> {
        let cut = || corrupt("it is shorter than its header");
        let (&[format, code, c0, c1], rest) = bytes.split_first_chunk::<4>().ok_or_else(cut)?;
        let (max, rest) = rest.split_first_chunk::<FLOAT32_BYTES>().ok_or_else(cut)?;
        let (min, rest) = rest.split_first_chunk::<FLOAT32_BYTES>().ok_or_else(cut)?;
        let (error, rest) = rest.split_first_chunk::<FLOAT32_BYTES>().ok_or_else(cut)?;
        let (&bits, _) = rest.split_first().ok_or_else(cut)?;
        if format != BLOCK_FORMAT {
            return Err(corrupt("its format is not 1"));
        }
        let quantization = Quantization::of_code(code)?;
        let count = usize::from(u16::from_le_bytes([c0, c1]));
        let (max, min, error) = (f32::from_le_bytes(*max), f32::from_le_bytes(*min), f32::from_le_bytes(*error));
        if count == 0 || !(max.is_finite() && min.is_finite() && min <= max && error.is_finite() && error >= 0.0_f32) {
            return Err(corrupt("its count or bounds are invalid"));
        }
        let mut reader = BitReader { bytes: bytes.get(HEADER_BYTES..).unwrap_or_default(), at: 0, acc: 0, filled: 0 };
        let first = reader.leb128()?;
        let last = first.checked_add(reader.leb128()?).ok_or_else(|| corrupt("its last row overflows"))?;
        let bits = u32::from(bits);
        if bits > ROW_BITS {
            return Err(corrupt("its row deltas are wider than a row"));
        }
        let deltas = HEADER_BYTES.saturating_add(reader.at);
        Ok(Header { count, first, last, max, min, error, quantization, bits, deltas })
    }
}

/// A block's postings, decoded: rows ascending, each with its weight as
/// stored, in double precision; the bounds of the weights inserted.
#[derive(Debug, Clone, PartialEq)]
pub struct Postings {
    /// The rows, ascending; a row twice is two versions'.
    pub rows: Vec<u64>,
    /// The weights, as stored.
    pub weights: Vec<f64>,
    /// At least every weight inserted.
    pub max: f32,
    /// At most every weight inserted.
    pub min: f32,
    /// At least how far a stored weight is from its weight inserted.
    pub error: f64,
}

impl Postings {
    /// One posting.
    pub fn single(row: u64, weight: f32) -> Postings {
        Postings { rows: vec![row], weights: vec![f64::from(weight)], max: weight, min: weight, error: 0.0_f64 }
    }

    /// Its postings.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Adds `row`'s posting of `weight`, after any of the row already.
    pub fn insert(&mut self, row: u64, weight: f32) {
        let at = self.rows.partition_point(|&r| r <= row);
        self.rows.insert(at, row);
        self.weights.insert(at, f64::from(weight));
        self.max = self.max.max(weight);
        self.min = self.min.min(weight);
    }

    /// Takes away `row`'s posting whose weight is nearest `weight`, if it
    /// has one: of two versions' postings of the row, the one inserted with
    /// it, or one alike. Whether it had one.
    pub fn remove(&mut self, row: u64, weight: f32) -> bool {
        let start = self.rows.partition_point(|&r| r < row);
        let end = self.rows.partition_point(|&r| r <= row);
        let wanted = f64::from(weight);
        let nearest = (start..end).min_by(|&a, &b| {
            let distance = |at: usize| self.weights.get(at).map_or(f64::INFINITY, |w| (w - wanted).abs());
            distance(a).total_cmp(&distance(b))
        });
        let Some(at) = nearest else { return false };
        self.rows.remove(at);
        self.weights.remove(at);
        true
    }

    /// Where it may be cut in two nearest its middle, never between two
    /// postings of one row, so a reader skipping rows it has passed never
    /// skips a version's; none if every posting is one row's.
    pub fn halfway(&self) -> Option<usize> {
        let middle = self.rows.len().checked_div(2)?;
        let parts = |at: usize| at > 0 && at < self.rows.len() && self.rows.get(at.wrapping_sub(1)) != self.rows.get(at);
        (0..=middle).find_map(|step| [middle.checked_add(step), middle.checked_sub(step)].into_iter().flatten().find(|&at| parts(at)))
    }

    /// Cuts it at `at`: the postings before, and those from it, each with
    /// its bounds and error.
    pub fn split(mut self, at: usize) -> (Postings, Postings) {
        let rows = self.rows.split_off(at.min(self.rows.len()));
        let weights = self.weights.split_off(at.min(self.weights.len()));
        let upper = Postings { rows, weights, max: self.max, min: self.min, error: self.error };
        (self, upper)
    }

    /// It and `next`, whose rows are all past its, as one.
    pub fn merge(mut self, next: Postings) -> Postings {
        self.rows.extend(next.rows);
        self.weights.extend(next.weights);
        self.max = self.max.max(next.max);
        self.min = self.min.min(next.min);
        self.error = self.error.max(next.error);
        self
    }

    /// The block `bytes`, decoded.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not a block of this format.
    pub fn decode(bytes: &[u8]) -> Result<Postings> {
        let header = Header::read(bytes)?;
        let mut reader = BitReader { bytes: bytes.get(header.deltas..).unwrap_or_default(), at: 0, acc: 0, filled: 0 };
        let mut row = header.first;
        let mut rows = Vec::with_capacity(header.count);
        rows.push(row);
        for _ in 1..header.count {
            row = row.checked_add(reader.bits(header.bits)?).ok_or_else(|| corrupt("a row overflows"))?;
            rows.push(row);
        }
        if row != header.last {
            return Err(corrupt("its last row is not its header's"));
        }
        let start = header.deltas.saturating_add(reader.at);
        let stored = bytes.get(start..).unwrap_or_default();
        if stored.len() != header.count.saturating_mul(header.quantization.width()) {
            return Err(corrupt("its length is not its count's"));
        }
        let weights = weights_of(stored, &header);
        Ok(Postings { rows, weights, max: header.max, min: header.min, error: f64::from(header.error) })
    }

    /// The block of these postings, its weights stored as `quantization`
    /// says, its error the error inherited and what storing them adds.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it has no postings or more than a block holds.
    pub fn encode(&self, quantization: Quantization) -> Result<Vec<u8>> {
        let (Some(&first), Some(&last)) = (self.rows.first(), self.rows.last()) else {
            return Err(SqlError::new(Code::Api, "[DESIGN] a posting block has at least one posting"));
        };
        let count = u16::try_from(self.rows.len())
            .map_err(|_| SqlError::new(Code::Api, "[DESIGN] a posting block has at most 65535 postings"))?;
        let deltas: Vec<u64> = self.rows.windows(2).map(|w| w.get(1).zip(w.first()).map_or(0, |(b, a)| b.wrapping_sub(*a))).collect();
        let bits = deltas.iter().max().map_or(0, |&m| ROW_BITS.saturating_sub(m.leading_zeros()));
        let (stored, measured) = stored_weights(self, quantization);
        let error = round_up(self.error + measured);
        let mut out = Vec::with_capacity(HEADER_BYTES.saturating_add(stored.len()).saturating_add(deltas.len()));
        out.extend_from_slice(&[BLOCK_FORMAT, quantization.code()]);
        out.extend_from_slice(&count.to_le_bytes());
        for bound in [self.max, self.min, error] {
            out.extend_from_slice(&bound.to_le_bytes());
        }
        out.push(u8::try_from(bits).unwrap_or(u8::MAX));
        let mut writer = BitWriter { out, acc: 0, filled: 0 };
        writer.leb128(first);
        writer.leb128(last.wrapping_sub(first));
        for delta in deltas {
            writer.bits(delta, bits);
        }
        let mut block = writer.finish();
        block.extend_from_slice(&stored);
        Ok(block)
    }
}

/// The step of UINT8 weights from `min` to `max`, and the scale of FLOAT16
/// ones: never zero.
fn scales(min: f32, max: f32) -> (f64, f64) {
    let step = (f64::from(max) - f64::from(min)) / BYTE_STEPS;
    let scale = f64::from(max.abs().max(min.abs()));
    (if step > 0.0_f64 { step } else { 1.0_f64 }, if scale > 0.0_f64 { scale } else { 1.0_f64 })
}

/// The weights of `stored`, of a block whose header is `header`.
fn weights_of(stored: &[u8], header: &Header) -> Vec<f64> {
    let (step, scale) = scales(header.min, header.max);
    let base = f64::from(header.min);
    match header.quantization {
        Quantization::Float32 => {
            stored.as_chunks::<FLOAT32_BYTES>().0.iter().map(|&b| f64::from(f32::from_le_bytes(b))).collect()
        }
        Quantization::Float16 => {
            stored.as_chunks::<FLOAT16_BYTES>().0.iter().map(|&b| f64::from(from_half(u16::from_le_bytes(b))) * scale).collect()
        }
        Quantization::Uint8 => stored.iter().map(|&q| base + f64::from(q) * step).collect(),
    }
}

/// The stored bytes of `postings`' weights, and the most a stored weight
/// is from the weight it stores.
fn stored_weights(postings: &Postings, quantization: Quantization) -> (Vec<u8>, f64) {
    let (step, scale) = scales(postings.min, postings.max);
    let base = f64::from(postings.min);
    let mut out = Vec::with_capacity(postings.weights.len().saturating_mul(quantization.width()));
    let mut measured = 0.0_f64;
    for &weight in &postings.weights {
        let back = match quantization {
            Quantization::Float32 => {
                let single = narrow(weight);
                out.extend_from_slice(&single.to_le_bytes());
                f64::from(single)
            }
            Quantization::Float16 => {
                let half = to_half(narrow(weight / scale));
                out.extend_from_slice(&half.to_le_bytes());
                f64::from(from_half(half)) * scale
            }
            Quantization::Uint8 => {
                let code = byte_of(((weight - base) / step).round());
                out.push(code);
                base + f64::from(code) * step
            }
        };
        measured = measured.max((weight - back).abs());
    }
    (out, measured)
}

/// `x`, a whole number, clamped to a byte.
fn byte_of(x: f64) -> u8 {
    const OFFSET: f64 = 4_503_599_627_370_496.0_f64;
    let clamped = x.clamp(0.0_f64, BYTE_STEPS);
    // Adding 2^52 leaves a whole number below 2^52 in the low mantissa bits.
    u8::try_from((clamped + OFFSET).to_bits() & 0xff).unwrap_or(u8::MAX)
}

/// The least single at least `x`, which is finite and not negative; its
/// greatest finite single past that.
fn round_up(x: f64) -> f32 {
    let near = narrow(x);
    if f64::from(near) < x { near.next_up().min(f32::MAX) } else { near }
}

/// Bits written low first into bytes.
struct BitWriter {
    out: Vec<u8>,
    acc: u128,
    filled: u32,
}

impl BitWriter {
    fn bits(&mut self, value: u64, bits: u32) {
        if bits == 0 {
            return;
        }
        let masked = if bits >= ROW_BITS { value } else { value & 1_u64.checked_shl(bits).map_or(u64::MAX, |b| b.wrapping_sub(1)) };
        self.acc |= u128::from(masked).checked_shl(self.filled).unwrap_or(0);
        self.filled = self.filled.saturating_add(bits);
        while self.filled >= BYTE_BITS {
            self.out.push(u8::try_from(self.acc & 0xff).unwrap_or_default());
            self.acc >>= BYTE_BITS;
            self.filled = self.filled.wrapping_sub(BYTE_BITS);
        }
    }

    fn leb128(&mut self, mut value: u64) {
        loop {
            let low = u8::try_from(value & 0x7f).unwrap_or_default();
            value >>= LEB_BITS;
            if value == 0 {
                self.out.push(low);
                return;
            }
            self.out.push(low | LEB_MORE);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.filled > 0 {
            self.out.push(u8::try_from(self.acc & 0xff).unwrap_or_default());
        }
        self.out
    }
}

/// Bits read low first from bytes.
struct BitReader<'a> {
    bytes: &'a [u8],
    /// The bytes taken.
    at: usize,
    acc: u128,
    filled: u32,
}

impl BitReader<'_> {
    fn bits(&mut self, bits: u32) -> Result<u64> {
        if bits == 0 {
            return Ok(0);
        }
        while self.filled < bits {
            let byte = *self.bytes.get(self.at).ok_or_else(|| corrupt("its row deltas are cut short"))?;
            self.acc |= u128::from(byte).checked_shl(self.filled).unwrap_or(0);
            self.at = self.at.saturating_add(1);
            self.filled = self.filled.saturating_add(BYTE_BITS);
        }
        let mask = 1_u128.checked_shl(bits).map_or(u128::MAX, |b| b.wrapping_sub(1));
        let value = u64::try_from(self.acc & mask).map_err(|_| corrupt("a row delta is too wide"))?;
        self.acc >>= bits;
        self.filled = self.filled.wrapping_sub(bits);
        Ok(value)
    }

    fn leb128(&mut self) -> Result<u64> {
        let mut value = 0_u64;
        let mut shift = 0_u32;
        loop {
            let byte = *self.bytes.get(self.at).ok_or_else(|| corrupt("its first row is cut short"))?;
            self.at = self.at.saturating_add(1);
            let part = u64::from(byte & !LEB_MORE).checked_shl(shift).filter(|_| shift < ROW_BITS);
            value |= part.ok_or_else(|| corrupt("its first row is too long"))?;
            if byte & LEB_MORE == 0 {
                return Ok(value);
            }
            shift = shift.saturating_add(LEB_BITS);
        }
    }
}
