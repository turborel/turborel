//! \[DESIGN\] A self-describing byte form of values and rows (RULES.md D25,
//! D31), the same on every platform: each value a tag, then its bytes. For
//! formats that stand alone, such as log records, which must be read
//! without the catalog's column types; the row format of the storage engine
//! (D31), which knows them, will be its own.
//!
//! ```text
//! value: 0 (null) | 1 (character) string | 2 (exact) units i128, scale u32 |
//!        3 (approximate) IEEE-754 bits u64
//! row:   values u32, then each value
//! ```

use super::{Decimal, Value};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::storage::Row;

/// The tags of the kinds of value.
const NULL: u8 = 0;
const CHARACTER: u8 = 1;
const EXACT: u8 = 2;
const APPROXIMATE: u8 = 3;

/// Appends `value` to `out`.
pub(crate) fn put_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => out.put_u8(NULL),
        Value::Char(s) => {
            out.put_u8(CHARACTER);
            out.put_str(s);
        }
        Value::Exact(d) => {
            out.put_u8(EXACT);
            out.put_i128(d.units);
            out.put_u32(d.scale);
        }
        Value::Approx(f) => {
            out.put_u8(APPROXIMATE);
            // The bits, so every value, NaN and -0.0 included, reads back as
            // it was.
            out.put_u64(f.to_bits());
        }
    }
}

/// Reads a value written by `put_value`.
pub(crate) fn value(reader: &mut Reader<'_>) -> Result<Value> {
    match reader.u8()? {
        NULL => Ok(Value::Null),
        CHARACTER => Ok(Value::Char(reader.str()?)),
        EXACT => {
            let units = reader.i128()?;
            Ok(Value::Exact(Decimal::new(units, reader.u32()?)))
        }
        APPROXIMATE => Ok(Value::Approx(f64::from_bits(reader.u64()?))),
        tag => Err(reader.corrupt(&format!("no kind of value has tag {tag}"))),
    }
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
