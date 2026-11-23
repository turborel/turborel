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
//!        12 (decimal floating point) coefficient i128, exponent i32 |
//!        13 (array) | 14 (row) | 17 (multiset): its parts u32, then each
//!          part's value |
//!        18 (structured) its type's schema and name, its attributes as a
//!          row's parts, then 0, or 1 and its key |
//!        19 (reference) its octets, their count u32 first |
//!        20 (JSON) its stored form, its count u32 first |
//!        15 (character large object) | 16 (binary large object): 0, then
//!          its octets, their count u32 first, if its row holds it; or 1,
//!          then the id its table keeps it by u64, its octets u64 and its
//!          characters u64 (RULES.md D79) |
//!        21 (XML) its stored form, as a binary large object's (RULES.md
//!          D106)
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
/// An array and a row: their parts (`types::composite`).
const ARRAY: u8 = 13;
const ROW: u8 = 14;
/// [ISO/IEC 9075-2:2023 §4.3] A character large object.
const CLOB: u8 = 15;
/// [ISO/IEC 9075-2:2023 §4.4] A binary large object.
const BLOB: u8 = 16;
/// [ISO/IEC 9075-2:2023 §4.12.3] A multiset: its elements, as an array's.
const MULTISET: u8 = 17;
/// [ISO/IEC 9075-2:2023 §4.9.3] A value of a structured type: its type's
/// name, then its attributes (`types::structured`).
const STRUCTURED: u8 = 18;
/// [ISO/IEC 9075-2:2023 §4.11] A reference: its octets.
const REFERENCE: u8 = 19;
/// [ISO/IEC 9075-2:2023 §4.8] A value of the JSON type: its stored form.
const JSON: u8 = 20;
/// [ISO/IEC 9075-14:2006 draft §4.2.3] An XML value: its stored form, as a
/// binary large object's (`types::xml`).
const XML: u8 = 21;

/// \[DESIGN\] Appends the character string of the UTF-8 bytes `text`
/// followed by `pad` spaces, as `put_value` appends it as a value: for a
/// writer that holds the string's bytes, not a value of it (RULES.md
/// D916).
pub(crate) fn put_spaced(out: &mut Vec<u8>, text: &[u8], pad: usize) {
    out.put_u8(CHARACTER);
    out.put_u32(u32::try_from(text.len().saturating_add(pad)).unwrap_or(u32::MAX));
    out.extend_from_slice(text);
    out.resize(out.len().saturating_add(pad), b' ');
}

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
        Value::Lob(l) => match l.kind() {
            super::lob::LobKind::Character => CLOB,
            super::lob::LobKind::Binary => BLOB,
        },
        Value::Bool(_) => BOOL,
        Value::Datetime(d) => match d.moment {
            Moment::Date => DATE,
            Moment::Time => TIME,
            Moment::Timestamp => TIMESTAMP,
        },
        Value::Interval(_) => INTERVAL,
        Value::Array(_) => ARRAY,
        Value::Multiset(_) => MULTISET,
        Value::Row(_) => ROW,
        Value::Structured(_) => STRUCTURED,
        Value::Reference(_) => REFERENCE,
        Value::Json(_) => JSON,
        Value::Xml(_) => XML,
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
        ARRAY => &types::ARRAY,
        MULTISET => &types::MULTISET,
        ROW => &types::ROW,
        STRUCTURED => &types::STRUCTURED,
        REFERENCE => &types::REFERENCE,
        JSON => &types::JSON,
        CLOB => &types::CLOB,
        BLOB => &types::BLOB,
        XML => &types::XML,
        EXTENSION => {
            let name = reader.str()?;
            reader.kind(&name)?
        }
        tag => return Err(reader.corrupt(&format!("no kind of value has tag {tag}"))),
    };
    kind.value(reader)
}

/// The bytes of a tag.
const TAG_LEN: usize = size_of::<u8>();
/// The bytes of a length before a string's or a row's bytes (`Put::put_bytes`).
const LENGTH_LEN: usize = size_of::<u32>();
/// The bytes of an exact number of a word: its tag, its units, its scale.
const EXACT_WORD_LEN: usize = TAG_LEN + size_of::<u64>() + size_of::<u8>();
/// The bytes of any other exact number: its tag, its units, its scale.
const EXACT_LEN: usize = TAG_LEN + size_of::<i128>() + size_of::<u32>();

/// \[DESIGN\] The bytes `put_value` writes of `value`: exactly, of a null,
/// an exact number, a character or binary string and a truth value, the
/// kinds most rows hold; of any other, its tag alone, fewer than it
/// writes. A record reserves this much, so it is written into one
/// allocation, not grown value by value.
pub(crate) fn value_len(value: &Value) -> usize {
    match value {
        Value::Exact(d) => {
            if i64::try_from(d.units).is_ok() && u8::try_from(d.scale).is_ok() {
                EXACT_WORD_LEN
            } else {
                EXACT_LEN
            }
        }
        Value::Char(s) => TAG_LEN.saturating_add(LENGTH_LEN).saturating_add(s.len()),
        Value::Binary(b) => TAG_LEN.saturating_add(LENGTH_LEN).saturating_add(b.bytes().len()),
        Value::Bool(_) => TAG_LEN.saturating_add(TAG_LEN),
        Value::Null
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Lob(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => TAG_LEN,
    }
}

/// The bytes `put_row` writes of `row`, as `value_len` counts its values.
pub(crate) fn row_len(row: &Row) -> usize {
    row.iter().fold(LENGTH_LEN, |n, v| n.saturating_add(value_len(v)))
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

    /// Verifies: [DESIGN] the bytes counted of a value are those written of
    /// a null, an exact number of either form, a character string, a
    /// binary string and a truth value, and never more than written of
    /// another kind; a row's are its count's and its values'.
    #[test]
    fn value_lengths_are_the_bytes_written() {
        let exact = [Value::Null, Value::Exact(Decimal::new(5, 2)), Value::Exact(Decimal::new(i128::MAX, 0))];
        let strings = [Value::Char("abc".into()), Value::Binary(crate::value::Binary::new(vec![1, 2])), Value::Bool(true)];
        let written = |v: &Value| {
            let mut out = Vec::new();
            put_value(&mut out, v);
            out.len()
        };
        for v in exact.iter().chain(&strings) {
            assert_eq!(value_len(v), written(v), "{v:?}");
        }
        let other = Value::Approx(1.5);
        assert!(value_len(&other) <= written(&other));
        let row: Row = exact.into_iter().chain(strings).collect();
        let mut rows = Vec::new();
        put_row(&mut rows, &row);
        assert_eq!(row_len(&row), rows.len());
    }
}
