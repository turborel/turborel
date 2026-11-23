//! \[DESIGN\] A record's columns decoded as they are listed (RULES.md
//! D862): the values of the columns asked for, in the order asked, appended
//! to a row the caller holds, so a read of a few columns of a wide table
//! makes no row of the table's width, and one read after another reuses
//! the row's room. The record is checked once, and each column is then
//! found from the format alone, the others never looked at.

use super::{Field, RecordFormat, Reference, corrupt, decode_value};
use crate::error::Result;
use crate::value::Value;

impl RecordFormat {
    /// Appends to `out` the value of each column of `columns` of a record,
    /// in their order, got by `fetch` if it is out of line: NULL for one
    /// that is not a column of the record (RULES.md D751). No other column
    /// is decoded, and no value is made for one.
    ///
    /// # Errors
    ///
    /// As `decode_with`; `out` is then as it was.
    pub fn decode_listed_with(
        &self,
        bytes: &[u8],
        columns: &[usize],
        fetch: &mut impl FnMut(Reference) -> Result<Vec<u8>>,
        out: &mut Vec<Value>,
    ) -> Result<()> {
        self.check(bytes)?;
        let base = out.len();
        out.reserve(columns.len());
        for &i in columns {
            let value = if i < self.types.len() { self.value_in(bytes, i, fetch) } else { Ok(Value::Null) };
            match value {
                Ok(value) => out.push(value),
                Err(e) => {
                    out.truncate(base);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Column `i` of a record already checked (`check`), got by `fetch` if
    /// it is out of line.
    pub(super) fn value_in(&self, bytes: &[u8], i: usize, fetch: &mut impl FnMut(Reference) -> Result<Vec<u8>>) -> Result<Value> {
        let data_type = self.types.get(i).ok_or_else(|| corrupt(&format!("it has no column {i}")))?;
        let unfit = || corrupt(&format!("column {i} is not a value of its type"));
        match self.field_in(bytes, i)? {
            Field::Null => Ok(Value::Null),
            Field::Inline(field) => decode_value(field, data_type).ok_or_else(unfit),
            Field::Outside(reference) => {
                let fetched = fetch(reference)?;
                if fetched.len() != reference.length {
                    return Err(corrupt(&format!("column {i}'s value out of line is not of the length its reference says")));
                }
                decode_value(&fetched, data_type).ok_or_else(unfit)
            }
        }
    }
}
