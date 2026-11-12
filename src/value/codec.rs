//! \[DESIGN\] A self-describing byte form of values and rows (RULES.md D25,
//! D31), the same on every platform: each value a tag, then its bytes. For
//! formats that stand alone, such as log records, which must be read
//! without the catalog's column types; the record format of the storage
//! engine (D31, `engine/record.rs`), which knows them, is its own.
//!
//! ```text
//! value: 0 (null) | 1 (character) string | 2 (exact) units i128, scale u32 |
//!        10 (exact, its units within 64 bits and its scale within 8) units
//!          i64, scale u8: so most take 10 bytes, not 21 |
//!        3 (approximate) IEEE-754 bits u64 |
//!        4 (beyond the standard) its kind's name, then its kind's bytes |
//!        5 (binary string) its octets, their count u32 first |
//!        6 (date) | 7 (time) | 8 (timestamp): with time zone u8, precision u8,
//!          displacement in minutes u32, microseconds in UTC u64 |
//!        9 (interval) its fields' ranks, then its precisions, each u32,
//!          then its units i128 |
//!        11 (truth value) 0 for False, 1 for True u8 |
//!        12 (decimal floating point) coefficient i128, exponent i32
//! row:   values u32, then each value
//! ```

use super::datetime::Moment;
use super::{SqlType, Value, types};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::storage::Row;

/// The tags of the kinds of value.
const NULL: u8 = 0;
const CHARACTER: u8 = 1;
const EXACT: u8 = 2;
const APPROXIMATE: u8 = 3;
/// A value of a kind beyond the standard: its kind's name, then its bytes.
const EXTENSION: u8 = 4;
/// A binary string: its octets.
const BINARY: u8 = 5;
/// A date, a time and a timestamp: as their kinds write them.
const DATE: u8 = 6;
const TIME: u8 = 7;
const TIMESTAMP: u8 = 8;
/// An interval: its qualifier, then its units.
const INTERVAL: u8 = 9;
/// An exact number whose units fit 64 bits and scale 8: its units, then its
/// scale.
const EXACT_WORD: u8 = 10;
/// A truth value: its byte.
const BOOL: u8 = 11;
/// A decimal floating-point number: its coefficient, then its exponent.
const DECFLOAT: u8 = 12;

/// Appends `value` to `out`: its tag, then its bytes, as its kind writes
/// them.
pub(crate) fn put_value(out: &mut Vec<u8>, value: &Value) {
    if let Value::Exact(d) = value
        && let (Ok(units), Ok(scale)) = (i64::try_from(d.units), u8::try_from(d.scale))
    {
        out.put_u8(EXACT_WORD);
        out.put_u64(units.cast_unsigned());
        out.put_u8(scale);
        return;
    }
    let tag = match value {
        Value::Null => NULL,
        Value::Char(_) => CHARACTER,
        Value::Exact(_) => EXACT,
        Value::Approx(_) => APPROXIMATE,
        Value::DecFloat(_) => DECFLOAT,
        Value::Binary(_) => BINARY,
        Value::Bool(_) => BOOL,
        Value::Datetime(d) => match d.moment {
            Moment::Date => DATE,
            Moment::Time => TIME,
            Moment::Timestamp => TIMESTAMP,
        },
        Value::Interval(_) => INTERVAL,
        Value::Extension(e) => {
            out.put_u8(EXTENSION);
            out.put_str(e.kind.name());
            e.kind.put_value(value, out);
            return;
        }
    };
    out.put_u8(tag);
    if let Some(kind) = value.kind() {
        kind.put_value(value, out);
    }
}

/// Reads a value written by `put_value`: of a kind beyond the standard,
/// found among the reader's types (`Reader::with_types`).
pub(crate) fn value(reader: &mut Reader<'_>) -> Result<Value> {
    let kind: &dyn SqlType = match reader.u8()? {
        NULL => return Ok(Value::Null),
        EXACT_WORD => {
            let units = reader.u64()?.cast_signed();
            return Ok(Value::Exact(super::Decimal::new(i128::from(units), u32::from(reader.u8()?))));
        }
        CHARACTER => &types::CHARACTER,
        EXACT => &types::EXACT,
        APPROXIMATE => &types::APPROXIMATE,
        DECFLOAT => &types::DECFLOAT,
        BINARY => &types::BINARY_VARYING,
        BOOL => &types::BOOLEAN,
        DATE => &types::DATE,
        TIME => &types::TIME,
        TIMESTAMP => &types::TIMESTAMP,
        INTERVAL => &types::INTERVAL_DAY_TIME,
        EXTENSION => {
            let name = reader.str()?;
            reader.kind(&name)?
        }
        tag => return Err(reader.corrupt(&format!("no kind of value has tag {tag}"))),
    };
    kind.value(reader)
}

/// Appends `row` to `out`.
pub(crate) fn put_row(out: &mut Vec<u8>, row: &Row) {
    out.put_u32(u32::try_from(row.len()).unwrap_or(u32::MAX));
    for v in row {
        put_value(out, v);
    }
}

/// Reads a row written by `put_row`.
pub(crate) fn row(reader: &mut Reader<'_>) -> Result<Row> {
    let count = reader.u32()?;
    // Pushed one by one, not allocated for `count` first: a corrupt count
    // fails at the first value missing, instead of asking for gigabytes.
    let mut row = Vec::new();
    for _ in 0..count {
        row.push(value(reader)?);
    }
    Ok(row)
}

// Unit tests: values and rows written and read back (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn read_back(bytes: &[u8]) -> Value {
        value(&mut Reader::new(bytes, "a test")).unwrap()
    }

    /// Verifies: [DESIGN] RULES.md D25, D38, the value codec: an exact
    /// number of units within 64 bits and a scale within 8 takes 10 bytes,
    /// one past either the longer form, each read back as written; a value
    /// of the longer form written before reads back too; a row of them
    /// reads back.
    #[test]
    fn exact_numbers_take_their_width() {
        for (units, scale, len) in
            [(1_234_i128, 2_u32, 10_usize), (-1, 0, 10), (i128::from(i64::MAX), 18, 10), (i128::MAX, 0, 21), (5, 300, 21)]
        {
            let v = Value::Exact(Decimal::new(units, scale));
            let mut out = Vec::new();
            put_value(&mut out, &v);
            assert_eq!(out.len(), len, "{units} at {scale}");
            assert_eq!(format!("{:?}", read_back(&out)), format!("{v:?}"));
        }
        let mut long = vec![EXACT];
        long.put_i128(-150);
        long.put_u32(2);
        assert_eq!(read_back(&long).to_string(), "-1.50", "the longer form");
        let row = vec![Value::Exact(Decimal::new(7, 1)), Value::Null, Value::Char("a".into())];
        let mut out = Vec::new();
        put_row(&mut out, &row);
        assert_eq!(format!("{:?}", super::row(&mut Reader::new(&out, "a test")).unwrap()), format!("{row:?}"));
    }
}
