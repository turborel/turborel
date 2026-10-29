//! \[DESIGN\] The column layout's chunks (RULES.md D31): the values of one
//! column of a frozen row group, a run of consecutive rows to a page, each
//! value as its type writes it in a record (`SqlType::encode_record`), so
//! every type, a plug-in's too, is kept.
//!
//! ```text
//! chunk:     kind u8 | count u32 | nulls, ceil(count / 8) bytes, a bit set
//!            for each null | values, as their encoding writes them
//!            (`encoded.rs`)
//! ```
//!
//! Integers are little-endian. Of the encodings that hold a chunk's values,
//! the one that takes fewest bytes is chosen, and a chunk holds as many
//! values as its page does so encoded; or, not compressed, fixed if its
//! type has a width in a record and every value is written in it, else
//! varying. Built on:
//!
//! * `group`: a frozen row group, its row ids, its chunks, the least and
//!   greatest value and nulls of each, and the pages they are on;
//! * `scan`: the rows of a group a reader asks for, chunks a condition
//!   cannot match left unread.

mod encoded;
pub mod group;
mod packed;
pub mod scan;

use crate::error::{Code, Result, SqlError};
use crate::value::{DataType, Value};

use encoded::{Body, Sizer};

/// Bytes of a count, a width or an offset.
const WORD: usize = 4;
/// Bytes before the nulls.
const HEADER: usize = 1 + WORD;
const BITS: usize = 8;

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a column chunk is malformed: {why}"))
}

fn too_large() -> SqlError {
    SqlError::new(Code::Unsupported, "[DESIGN] a column chunk would be too large")
}

/// The byte of a null bitmap that value `n`'s bit is in, and the bit.
fn bit_of(n: usize) -> (usize, u8) {
    (n.checked_div(BITS).unwrap_or(0), u8::try_from(n.checked_rem(BITS).unwrap_or(0)).unwrap_or(0))
}

/// Bytes of the null bitmap of `count` values.
fn nulls_len(count: usize) -> usize {
    count.div_ceil(BITS)
}

fn word(n: usize) -> Result<[u8; WORD]> {
    u32::try_from(n).map(u32::to_le_bytes).map_err(|_| too_large())
}

fn read_word(bytes: &[u8], at: usize) -> Result<usize> {
    let end = at.checked_add(WORD).ok_or_else(|| corrupt("it is too short"))?;
    let b = bytes.get(at..end).and_then(|b| <[u8; WORD]>::try_from(b).ok()).ok_or_else(|| corrupt("it is too short"))?;
    usize::try_from(u32::from_le_bytes(b)).map_err(|_| corrupt("a count is too large"))
}

/// The width every value of `t` in `fields` is written in, if they all are.
fn width_of(fields: &[Option<Vec<u8>>], t: &DataType) -> Option<usize> {
    let width = t.kind().record_width(t)?;
    fields.iter().flatten().all(|f| f.len() == width).then_some(width)
}

/// The values of a column, each as its type writes it in a record, none
/// for a null.
pub struct Fields(Vec<Option<Vec<u8>>>);

impl Fields {
    /// `values`, of type `t`, written.
    ///
    /// # Errors
    ///
    /// As `SqlType::encode_record` says.
    pub fn of(t: &DataType, values: impl Iterator<Item = Value>) -> Result<Fields> {
        values.map(|v| if v.is_null() { Ok(None) } else { t.encode_record(&v).map(Some) }).collect::<Result<_>>().map(Fields)
    }

    /// How many there are.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many of those from `from` on fit a chunk of `capacity` bytes, of
    /// type `t`, compressed if `compress`; at least one, so a value longer
    /// than a chunk has one of its own.
    pub fn fitting(&self, t: &DataType, from: usize, (capacity, compress): (usize, bool)) -> usize {
        let rest = self.0.get(from..).unwrap_or_default();
        let mut sizer = Sizer::new(width_of(&self.0, t), compress);
        for (n, field) in rest.iter().enumerate() {
            sizer.add(field.as_deref());
            if sizer.best().1 > capacity {
                return n.max(1);
            }
        }
        rest.len()
    }

    /// The chunk of those from `from`, `count` of them, of type `t`, in the
    /// encoding that takes fewest bytes, compressed if `compress`.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if it is longer than a count describes.
    pub fn chunk(&self, t: &DataType, from: usize, (count, compress): (usize, bool)) -> Result<Vec<u8>> {
        let end = from.checked_add(count).ok_or_else(too_large)?;
        let fields = self.0.get(from..end).ok_or_else(|| corrupt("its values are out of range"))?;
        let mut sizer = Sizer::new(width_of(&self.0, t), compress);
        for f in fields {
            sizer.add(f.as_deref());
        }
        let (plan, _) = sizer.best();
        let mut out = vec![encoded::kind_of(plan)];
        out.extend_from_slice(&word(count)?);
        let mut nulls = vec![0_u8; nulls_len(count)];
        for (n, field) in fields.iter().enumerate() {
            let (byte, bit) = bit_of(n);
            if let Some(b) = nulls.get_mut(byte).filter(|_| field.is_none()) {
                *b |= 1 << bit;
            }
        }
        out.extend_from_slice(&nulls);
        encoded::encode(plan, fields, &mut out)?;
        Ok(out)
    }
}

/// A chunk, read in place.
pub struct Chunk<'a> {
    count: usize,
    nulls: &'a [u8],
    body: Body<'a>,
}

impl<'a> Chunk<'a> {
    /// The chunk `bytes` hold.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not one.
    pub fn parse(bytes: &'a [u8]) -> Result<Chunk<'a>> {
        let kind = bytes.first().copied().ok_or_else(|| corrupt("it is empty"))?;
        let count = read_word(bytes, 1)?;
        let nulls_end = HEADER.checked_add(nulls_len(count)).ok_or_else(|| corrupt("its count is too large"))?;
        let nulls = bytes.get(HEADER..nulls_end).ok_or_else(|| corrupt("it is too short"))?;
        Ok(Chunk { count, nulls, body: encoded::body(kind, count, (bytes, nulls_end))? })
    }

    /// How many values it holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether its `n`th value is null.
    fn is_null(&self, n: usize) -> bool {
        let (byte, bit) = bit_of(n);
        self.nulls.get(byte).is_some_and(|b| b & (1 << bit) != 0)
    }

    /// \[DESIGN\] Its values at positions `from` to `to`, each an exact
    /// number's units, as integers pushed to `units`, and whether each is
    /// null to `nulls` (`encoded::Body::ints`): none if its encoding does
    /// not hold integers.
    pub fn ints(&self, (from, to): (usize, usize), (units, nulls): (&mut Vec<i64>, &mut Vec<bool>)) -> Option<()> {
        if to > self.count {
            return None;
        }
        self.body.ints((from, to), units)?;
        nulls.extend((from..to).map(|n| self.is_null(n)));
        Some(())
    }

    /// \[DESIGN\] Its values at positions `from` to `to`, character strings
    /// written as they are, as text pushed to `text` and where each ends to
    /// `ends`, and whether each is null to `nulls` (`encoded::Body::texts`):
    /// none if its encoding does not hold them as text.
    pub fn texts(&self, (from, to): (usize, usize), (text, ends, nulls): (&mut String, &mut Vec<usize>, &mut Vec<bool>)) -> Option<()> {
        if to > self.count {
            return None;
        }
        self.body.texts((from, to), (text, ends), |n| self.is_null(n))?;
        nulls.extend((from..to).map(|n| self.is_null(n)));
        Some(())
    }

    /// \[DESIGN\] Of a dictionary, its entries and the entry of each of
    /// `positions` pushed to `coded`, and whether each is null to `nulls`
    /// (`encoded::Body::coded`): none if its encoding is not a dictionary.
    pub fn coded(
        &self,
        positions: &[usize],
        (text, ends, codes, nulls): (&mut String, &mut Vec<usize>, &mut Vec<u32>, &mut Vec<bool>),
    ) -> Option<()> {
        if positions.iter().any(|&n| n >= self.count) {
            return None;
        }
        self.body.coded(positions.iter().copied(), (text, ends, codes), |n| self.is_null(n))?;
        nulls.extend(positions.iter().map(|&n| self.is_null(n)));
        Some(())
    }

    /// Its `n`th value, of type `t`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not there, or not one of `t`.
    pub fn value(&self, n: usize, t: &DataType) -> Result<Value> {
        if n >= self.count {
            return Err(corrupt("a value past its end is asked for"));
        }
        if self.is_null(n) {
            return Ok(Value::Null);
        }
        self.body.value(n, t)
    }
}

// Unit tests: chunks read back as written, fixed and varying, with nulls;
// how many values fit; malformed chunks refused (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests;
