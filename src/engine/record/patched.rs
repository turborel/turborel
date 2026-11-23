//! \[DESIGN\] A record made of another record's bytes (RULES.md D915). An
//! update that sets a few columns of a row leaves the others as they are,
//! and every column of a record is found from the format alone. So the
//! record of the row after is the record of the row before with the null
//! bits and the fixed fields of the columns set written again, and, where
//! one of them is of varying width, the ends and the variable bytes built
//! again, the spans of the columns kept copied as they lie. No column kept
//! is decoded and none is encoded again.
//!
//! The record so made is, byte for byte, the one `encode_spilling` writes
//! of the same row. A value set is written by the encoder that writes it
//! there (`inline::encode_into`), a null one leaves zeros in its fixed
//! field, and a record a spill would not keep whole is not made. Neither
//! is one of a record that keeps a value out of line, or that is not of
//! this format, such as one sealed. The caller then writes the row whole.

use super::inline::{encode_into, string_len};
use super::{END_BYTES, OUTSIDE, Place, RecordFormat, Spill, null_bit};
use crate::error::{Code, Result, SqlError};
use crate::storage::Row;
use crate::value::Value;

impl RecordFormat {
    /// Appends to `out` the record `old` with each column of `set` given
    /// its value in `row`, and true; or, if it is not made so, nothing,
    /// and false. It is not made of a record that is not of this format,
    /// one that keeps a value out of line, or one `spill` would not keep
    /// whole once changed. No other value of `row` is read.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `row` is not of the format's width, `set` names a
    /// column it has not, or a value set is not of its column's type;
    /// `out` is then as it was.
    pub fn patched(&self, old: &[u8], (row, set): (&Row, &[usize]), spill: Option<Spill>, out: &mut Vec<u8>) -> Result<bool> {
        self.check_len(row)?;
        if !self.all_in_line(old) {
            return Ok(false);
        }
        let base = out.len();
        // Room for the whole record, the strings set counted, so it is
        // not grown as it is written.
        let strings = set.iter().filter_map(|&i| row.get(i)).fold(0_usize, |n, v| n.saturating_add(string_len(v)));
        out.reserve(old.len().saturating_add(strings));
        if let Err(e) = self.put_patched(old, (row, set), (out, base)) {
            out.truncate(base);
            return Err(e);
        }
        if spill.is_none_or(|s| self.keeps_whole(out.get(base..).unwrap_or_default(), s)) {
            return Ok(true);
        }
        out.truncate(base);
        Ok(false)
    }

    /// Whether `record` is of this format with every value in line, its
    /// ends in order and its last the end of its bytes: one whose spans
    /// may be copied as they lie.
    fn all_in_line(&self, record: &[u8]) -> bool {
        if self.check(record).is_err() {
            return false;
        }
        let mut before = 0_usize;
        for n in 0..self.variable_columns {
            let Some(end) = self.end_of(record, n).filter(|&end| end >= before) else { return false };
            before = end;
        }
        self.variable_at().checked_add(before) == Some(record.len())
    }

    /// Where the bytes of the `n`-th column of varying width of `record`
    /// end, counted from the start of its variable bytes: none if its
    /// value is kept out of line, or the record has no such end.
    fn end_of(&self, record: &[u8], n: usize) -> Option<usize> {
        let at = self.ends_at().checked_add(n.checked_mul(END_BYTES)?)?;
        let end = u32::from_le_bytes(<[u8; END_BYTES]>::try_from(record.get(at..at.checked_add(END_BYTES)?)?).ok()?);
        if end & OUTSIDE != 0 {
            return None;
        }
        usize::try_from(end).ok()
    }

    /// Writes at `base` of `out` the record `old`, every value in line,
    /// with the columns `set` given their values in `row`.
    fn put_patched(&self, old: &[u8], (row, set): (&Row, &[usize]), (out, base): (&mut Vec<u8>, usize)) -> Result<()> {
        let variable_at = self.variable_at();
        out.extend_from_slice(old.get(..variable_at).unwrap_or_default());
        let mut varying = false;
        for &i in set {
            let (Some(value), Some(&place)) = (row.get(i), self.places.get(i)) else {
                return Err(SqlError::new(Code::Api, format!("[DESIGN] a table of {} columns has no column {i} to set", self.types.len())));
            };
            let (bit_at, mask) = null_bit(i);
            if let Some(byte) = out.get_mut(base.saturating_add(bit_at)) {
                *byte = if value.is_null() { *byte | mask } else { *byte & !mask };
            }
            match place {
                Place::Fixed(at, width) => self.put_fixed((i, value), (at, width), (out, base))?,
                Place::Variable(_) => varying = true,
            }
        }
        if !varying {
            out.extend_from_slice(old.get(variable_at..).unwrap_or_default());
            return Ok(());
        }
        let mut begin = variable_at;
        for (i, place) in self.places.iter().enumerate() {
            let Place::Variable(n) = *place else { continue };
            let end = variable_at.saturating_add(self.end_of(old, n).unwrap_or_default());
            match row.get(i).filter(|_| set.contains(&i)) {
                Some(value) => self.put_value((i, value), out)?,
                None => out.extend_from_slice(old.get(begin..end).unwrap_or_default()),
            }
            begin = end;
            self.put_end((out, base), n, 0)?;
        }
        Ok(())
    }

    /// Writes `value`, column `i`'s, in its fixed field of `width` bytes at
    /// `at` of the fixed part of the record at `base` of `out`: zeros for
    /// NULL, as a record encoded whole holds.
    fn put_fixed(&self, (i, value): (usize, &Value), (at, width): (usize, usize), (out, base): (&mut Vec<u8>, usize)) -> Result<()> {
        let from = base.saturating_add(self.fixed_at()).saturating_add(at);
        // Written past the record's end, then moved to its place.
        let tail = out.len();
        self.put_value((i, value), out)?;
        if let Some((record, bytes)) = out.split_at_mut_checked(tail)
            && let Some(field) = record.get_mut(from..from.saturating_add(width))
        {
            field.fill(0);
            if let Some(slot) = field.get_mut(..bytes.len()) {
                slot.copy_from_slice(bytes);
            }
        }
        out.truncate(tail);
        Ok(())
    }

    /// Appends to `out` the bytes of `value`, column `i`'s, as its type
    /// writes them: none of NULL.
    fn put_value(&self, (i, value): (usize, &Value), out: &mut Vec<u8>) -> Result<()> {
        match self.types.get(i).filter(|_| !value.is_null()) {
            Some(data_type) => encode_into(value, data_type, out),
            None => Ok(()),
        }
    }
}

// Unit tests: a record patched is the record encoded whole (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
