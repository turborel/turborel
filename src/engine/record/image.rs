//! \[DESIGN\] A record's values as an update's log record holds them
//! (RULES.md D916, D475). An update written as a patch holds the values
//! of the columns it set alone, and its log record holds the row after
//! whole. The values it did not set are in the record written, so the
//! image is made of the record. It holds each value as `value::codec`
//! writes it, the bytes it would write of the value decoded. A character
//! string, the one kind whose value is an allocation of its own, is
//! written from the record's bytes, a CHARACTER one with the spaces its
//! record leaves out. A value of any other kind is decoded and written.

use super::{Field, RecordFormat, corrupt, decode_value};
use crate::error::Result;
use crate::storage::AfterImage;
use crate::value::codec::{put_spaced, put_value};
use crate::value::{DataType, Value};

impl RecordFormat {
    /// Makes `image` the image of the row the record `bytes` holds, every
    /// value in line, an update of which set the columns `set`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `bytes` are not a record of this format, a value is
    /// not one of its column's type, or one is kept out of line.
    pub fn image(&self, bytes: &[u8], set: &[usize], image: &mut AfterImage) -> Result<()> {
        self.check(bytes)?;
        image.clear();
        for (i, data_type) in self.types.iter().enumerate() {
            let field = self.field_in(bytes, i)?;
            image.put(set.contains(&i), |out| put_field(&field, (i, data_type), out))?;
        }
        Ok(())
    }
}

/// Appends to `out` the value `field` holds, column `i`'s, of `data_type`,
/// as `value::codec` writes it.
fn put_field(field: &Field<'_>, (i, data_type): (usize, &DataType), out: &mut Vec<u8>) -> Result<()> {
    let unfit = || corrupt(&format!("column {i} is not a value of its type"));
    let bytes = match field {
        Field::Null => {
            put_value(out, &Value::Null);
            return Ok(());
        }
        Field::Inline(bytes) => *bytes,
        Field::Outside(_) => return Err(corrupt(&format!("column {i} is kept out of line"))),
    };
    // A string's pad: the spaces a CHARACTER column's record leaves out.
    let pad = match data_type {
        DataType::Character { length, units } => {
            let text = std::str::from_utf8(bytes).map_err(|_| unfit())?;
            Some(usize::try_from(*length).ok().and_then(|n| n.checked_sub(units.count(text))).ok_or_else(unfit)?)
        }
        DataType::Varying { .. } => std::str::from_utf8(bytes).map(|_| Some(0)).map_err(|_| unfit())?,
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
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
        | DataType::Xml(_)
        | DataType::Extension(_) => None,
    };
    match pad {
        Some(pad) => put_spaced(out, bytes, pad),
        None => put_value(out, &decode_value(bytes, data_type).ok_or_else(unfit)?),
    }
    Ok(())
}

// Unit tests: a record's image is its row's (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
