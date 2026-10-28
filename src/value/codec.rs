//! \[DESIGN\] A self-describing byte form of values and rows (RULES.md D25,
//! D31), the same on every platform: each value a tag, then its bytes. For
//! formats that stand alone, such as log records, which must be read
//! without the catalog's column types; the row format of the storage engine
//! (D31), which knows them, will be its own.
//!
//! ```text
//! value: 0 (null) | 1 (character) string | 2 (exact) units i128, scale u32 |
//!        3 (approximate) IEEE-754 bits u64 |
//!        4 (beyond the standard) its kind's name, then its kind's bytes |
//!        5 (bit string) its length in bits u32, then its bytes
//! row:   values u32, then each value
//! ```

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
/// A bit string: its length in bits, then its bytes.
const BITS: u8 = 5;

/// Appends `value` to `out`: its tag, then its bytes, as its kind writes
/// them.
pub(crate) fn put_value(out: &mut Vec<u8>, value: &Value) {
    let tag = match value {
        Value::Null => NULL,
        Value::Char(_) => CHARACTER,
        Value::Exact(_) => EXACT,
        Value::Approx(_) => APPROXIMATE,
        Value::Bits(_) => BITS,
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
        CHARACTER => &types::CHARACTER,
        EXACT => &types::EXACT,
        APPROXIMATE => &types::APPROXIMATE,
        BITS => &types::BIT_VARYING,
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
