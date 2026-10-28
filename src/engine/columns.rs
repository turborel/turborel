//! \[DESIGN\] The column layout's chunks (RULES.md D31): the values of one
//! column of a frozen row group, a run of consecutive rows to a page, each
//! value as its type writes it in a record (`SqlType::encode_record`), so
//! every type, a plug-in's too, is kept.
//!
//! ```text
//! chunk:     kind u8 | count u32 | nulls, ceil(count / 8) bytes, a bit set
//!            for each null | values
//! FIXED:     width u32 | count × width bytes, a null's zeroed
//! VARYING:   (count + 1) × u32, where each value begins and the last ends,
//!            from the start of the bytes | the bytes
//! ```
//!
//! Integers are little-endian. A chunk is fixed when its type has a width
//! in a record and every value is written in it, else varying. Built on:
//!
//! * `group`: a frozen row group, its row ids, its chunks, the least and
//!   greatest value and nulls of each, and the pages they are on;
//! * `scan`: the rows of a group a reader asks for, chunks a condition
//!   cannot match left unread.

pub mod group;
pub mod scan;

use crate::error::{Code, Result, SqlError};
use crate::value::{DataType, Value};

const FIXED: u8 = 1;
const VARYING: u8 = 2;
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
    /// type `t`; at least one, so a value longer than a chunk has one of its
    /// own.
    pub fn fitting(&self, t: &DataType, from: usize, capacity: usize) -> usize {
        let rest = self.0.get(from..).unwrap_or_default();
        let fixed = width_of(&self.0, t);
        // The width, or the first offset.
        let mut used = HEADER.saturating_add(WORD);
        for (n, field) in rest.iter().enumerate() {
            let len = fixed.unwrap_or_else(|| field.as_ref().map_or(0, Vec::len).saturating_add(WORD));
            used = used.saturating_add(len);
            if used.saturating_add(nulls_len(n.saturating_add(1))) > capacity {
                return n.max(1);
            }
        }
        rest.len()
    }

    /// The chunk of those from `from`, `count` of them, of type `t`.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if it is longer than a count describes.
    pub fn chunk(&self, t: &DataType, from: usize, count: usize) -> Result<Vec<u8>> {
        let end = from.checked_add(count).ok_or_else(too_large)?;
        let fields = self.0.get(from..end).ok_or_else(|| corrupt("its values are out of range"))?;
        let fixed = width_of(&self.0, t);
        let mut out = vec![if fixed.is_some() { FIXED } else { VARYING }];
        out.extend_from_slice(&word(count)?);
        let mut nulls = vec![0_u8; nulls_len(count)];
        for (n, field) in fields.iter().enumerate() {
            let (byte, bit) = bit_of(n);
            if let Some(b) = nulls.get_mut(byte).filter(|_| field.is_none()) {
                *b |= 1 << bit;
            }
        }
        out.extend_from_slice(&nulls);
        if let Some(width) = fixed {
            out.extend_from_slice(&word(width)?);
            for field in fields {
                match field {
                    Some(f) => out.extend_from_slice(f),
                    None => out.resize(out.len().saturating_add(width), 0),
                }
            }
            return Ok(out);
        }
        let mut at = 0_usize;
        out.extend_from_slice(&word(at)?);
        for field in fields {
            at = at.checked_add(field.as_ref().map_or(0, Vec::len)).ok_or_else(too_large)?;
            out.extend_from_slice(&word(at)?);
        }
        fields.iter().flatten().for_each(|f| out.extend_from_slice(f));
        Ok(out)
    }
}

/// A chunk, read in place.
pub struct Chunk<'a> {
    count: usize,
    nulls: &'a [u8],
    /// The width of each value, if fixed; else where each begins.
    width: Option<usize>,
    offsets: &'a [u8],
    values: &'a [u8],
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
        let rest_at = nulls_end.checked_add(WORD).ok_or_else(|| corrupt("it is too short"))?;
        match kind {
            FIXED => {
                let width = read_word(bytes, nulls_end)?;
                let len = count.checked_mul(width).ok_or_else(|| corrupt("its values are too long"))?;
                let values = rest_at.checked_add(len).and_then(|end| bytes.get(rest_at..end)).ok_or_else(|| corrupt("it is too short"))?;
                Ok(Chunk { count, nulls, width: Some(width), offsets: &[], values })
            }
            VARYING => {
                let offsets_len =
                    count.checked_add(1).and_then(|n| n.checked_mul(WORD)).ok_or_else(|| corrupt("its count is too large"))?;
                let offsets_end = nulls_end.checked_add(offsets_len).ok_or_else(|| corrupt("it is too short"))?;
                let offsets = bytes.get(nulls_end..offsets_end).ok_or_else(|| corrupt("it is too short"))?;
                let values = bytes.get(offsets_end..).unwrap_or_default();
                Ok(Chunk { count, nulls, width: None, offsets, values })
            }
            _ => Err(corrupt("it is of another format")),
        }
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
        let field = if let Some(width) = self.width {
            n.checked_mul(width).and_then(|at| self.values.get(at..at.checked_add(width)?))
        } else {
            let from = read_word(self.offsets, n.saturating_mul(WORD))?;
            let to = read_word(self.offsets, n.saturating_add(1).saturating_mul(WORD))?;
            self.values.get(from..to)
        };
        field.and_then(|f| t.decode_record(f)).ok_or_else(|| corrupt("a value is not one of its column's type"))
    }
}

// Unit tests: chunks read back as written, fixed and varying, with nulls;
// how many values fit; malformed chunks refused (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests;
