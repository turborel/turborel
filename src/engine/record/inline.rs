//! \[DESIGN\] A row's record written with every value in line (RULES.md
//! D31), each value's bytes put straight into the record by its kind
//! (`SqlType::encode_record_into`), none allocated apart. Most rows hold no
//! value long enough to go out of line, so `encode_spilling` writes a row
//! this way first, and measures and chooses only for one it does not keep
//! whole.

use super::{END_BYTES, HEADER, Place, RecordFormat, Spill, null_bit, put};
use crate::error::Result;
use crate::storage::Row;
use crate::value::types::{CHARACTER, EXACT, SqlType, VARYING};
use crate::value::{DataType, Value};

impl RecordFormat {
    /// Appends the record of `row` to `out`, every value in line, and
    /// true; or, if `spill` would put a value out of line, none, `out` as
    /// it was, and false. A value of varying width longer than its
    /// threshold goes out of line, as do the longest of a record longer
    /// than its most (`choose`), so a record of neither is kept whole.
    ///
    /// # Errors
    ///
    /// As `encode`; `out` then as it was.
    pub(super) fn encode_in_line(&self, row: &Row, spill: Option<Spill>, out: &mut Vec<u8>) -> Result<bool> {
        self.check_len(row)?;
        let base = out.len();
        // Room for the whole record, its strings' bytes counted, so it is
        // not grown as its values are written.
        out.reserve(self.in_line_room(row));
        if let Err(e) = self.put_in_line(row, (out, base)) {
            out.truncate(base);
            return Err(e);
        }
        if spill.is_none_or(|s| self.keeps_whole(out.get(base..).unwrap_or_default(), s)) {
            return Ok(true);
        }
        out.truncate(base);
        Ok(false)
    }

    /// The bytes the record of `row` takes at most with every value in
    /// line, its strings' bytes counted: the room a buffer is made with
    /// so it is not grown as the record is written into it.
    pub(in crate::engine) fn in_line_room(&self, row: &Row) -> usize {
        let strings = row.iter().fold(0_usize, |n, v| n.saturating_add(string_len(v)));
        self.variable_at().saturating_add(strings)
    }

    /// Writes the record of `row`, of as many values as its format's
    /// columns, at `base` of `out`, every value in line.
    fn put_in_line(&self, row: &Row, (out, base): (&mut Vec<u8>, usize)) -> Result<()> {
        out.resize(base.saturating_add(self.variable_at()), 0);
        if let Some(header) = out.get_mut(base) {
            *header = HEADER;
        }
        for (i, (value, (data_type, &place))) in row.iter().zip(self.types.iter().zip(&self.places)).enumerate() {
            if value.is_null() {
                let (at, mask) = null_bit(i);
                if let Some(byte) = out.get_mut(base.saturating_add(at)) {
                    *byte |= mask;
                }
            } else if let Place::Fixed(at, _) = place {
                // Written past the record's end, then moved to its place.
                let tail = out.len();
                encode_into(value, data_type, out)?;
                if let Some((record, bytes)) = out.split_at_mut_checked(tail) {
                    put(record.get_mut(base..).unwrap_or_default(), self.fixed_at().saturating_add(at), bytes);
                }
                out.truncate(tail);
            } else {
                encode_into(value, data_type, out)?;
            }
            if let Place::Variable(n) = place {
                self.put_end((out, base), n, 0)?;
            }
        }
        Ok(())
    }

    /// Whether `spill` keeps `record`, written in line, whole: no value of
    /// varying width longer than its threshold, and the record no longer
    /// than its most.
    pub(super) fn keeps_whole(&self, record: &[u8], spill: Spill) -> bool {
        if record.len() > spill.most {
            return false;
        }
        let mut before = 0_usize;
        for n in 0..self.variable_columns {
            let at = self.ends_at().saturating_add(n.saturating_mul(END_BYTES));
            let end = record.get(at..at.saturating_add(END_BYTES)).and_then(|b| <[u8; END_BYTES]>::try_from(b).ok());
            let Some(end) = end.map(u32::from_le_bytes).and_then(|e| usize::try_from(e).ok()) else { return false };
            if end.saturating_sub(before) > spill.threshold {
                return false;
            }
            before = end;
        }
        true
    }
}

/// The bytes of a string `v` is, none of another value: the most its
/// record holds of it.
pub(super) fn string_len(v: &Value) -> usize {
    match v {
        Value::Char(s) => s.len(),
        Value::Binary(b) => b.bytes().len(),
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => 0,
    }
}

/// Appends the bytes of non-null `value` in a column of `data_type` to
/// `out`, as its kind encodes them: a call to the kinds most columns are of
/// made directly, so it is inlined.
pub(super) fn encode_into(value: &Value, data_type: &DataType, out: &mut Vec<u8>) -> Result<()> {
    match data_type {
        DataType::Exact { .. } => EXACT.encode_record_into(value, data_type, out),
        DataType::Character { .. } => CHARACTER.encode_record_into(value, data_type, out),
        DataType::Varying { .. } => VARYING.encode_record_into(value, data_type, out),
        DataType::Xml(_)
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Lob { .. }
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Json
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Extension(_) => data_type.kind().encode_record_into(value, data_type, out),
    }
}
