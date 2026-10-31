//! \[DESIGN\] A batch gathered from records (RULES.md D31,
//! `TableStorage::scan_batches`): each column asked for decoded straight
//! from each record's field into its vector, no row made: an exact
//! number's units, written as an integer of 8 bytes (`record.rs`), into
//! integers (`storage::Ints`); a character string's text into one buffer
//! (`storage::Texts`), a CHARACTER string padded to its length as it is
//! read [X3.135-1986 §8.7 GR7b]; any other value decoded as a row's is.
//! So the row store's batches, and the column store's rows not yet
//! frozen, are of the vectors its frozen rows give. Split from scan.rs
//! (RULES.md R-SIZE-2).

use super::corrupt;
use crate::engine::columns::{padded, padding};
use crate::engine::record::{RecordFormat, Reference};
use crate::error::Result;
use crate::storage::{Batch, Column, Ints, Texts};
use crate::value::{DataType, Value};

/// Bytes of an exact number's field in a record.
const UNITS_BYTES: usize = 8;

/// A column's vector, as it is gathered.
enum Gathered {
    /// Exact numbers of `scale`, as integers.
    Ints { units: Vec<i64>, scale: u32, nulls: Nulls },
    /// Character strings, as their text, padded to `pad` characters if
    /// given.
    Texts { text: String, ends: Vec<usize>, pad: Option<usize>, nulls: Nulls },
    /// Values of any other type, or of a column the table lacks.
    Values(Vec<Value>),
}

/// Whether each row gathered is null, kept only from the first null on.
#[derive(Default)]
struct Nulls(Option<Vec<bool>>);

impl Nulls {
    /// Notes the next row, of `rows` before it, null or not.
    fn push(&mut self, rows: usize, null: bool) {
        match (&mut self.0, null) {
            (Some(held), _) => held.push(null),
            (None, true) => {
                let mut held = vec![false; rows];
                held.push(true);
                self.0 = Some(held);
            }
            (None, false) => {}
        }
    }
}

/// \[DESIGN\] The columns of a batch being gathered from records.
pub(in crate::engine::rows) struct Gathering {
    /// Each column asked for, and its vector.
    columns: Vec<(usize, Gathered)>,
    rows: usize,
}

impl Gathering {
    /// A batch of `columns` of a table of `types` to gather.
    pub(in crate::engine::rows) fn new(types: &[DataType], columns: &[usize]) -> Gathering {
        Gathering { columns: columns.iter().map(|&c| (c, Self::vector(types.get(c)))).collect(), rows: 0 }
    }

    /// The vector a column of type `t`, if the table has it, is gathered
    /// into.
    fn vector(t: Option<&DataType>) -> Gathered {
        match t {
            Some(DataType::Exact { scale, .. }) => Gathered::Ints { units: Vec::new(), scale: *scale, nulls: Nulls::default() },
            Some(other) => match padding(other) {
                Some(pad) => Gathered::Texts { text: String::new(), ends: Vec::new(), pad, nulls: Nulls::default() },
                None => Gathered::Values(Vec::new()),
            },
            None => Gathered::Values(Vec::new()),
        }
    }

    /// How many rows it holds.
    pub(in crate::engine::rows) fn rows(&self) -> usize {
        self.rows
    }

    /// Adds the row plain `record` of `format` holds, a value out of line
    /// got by `fetch`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a field is not a value of its column's type; as
    /// `RecordFormat::bytes_with`.
    pub(in crate::engine::rows) fn push(
        &mut self,
        (format, record): (&RecordFormat, &[u8]),
        fetch: &mut impl FnMut(Reference) -> Result<Vec<u8>>,
    ) -> Result<()> {
        let rows = self.rows;
        for (c, vector) in &mut self.columns {
            let at = *c;
            match vector {
                Gathered::Ints { units, nulls, .. } => {
                    let field = format.bytes_with(record, at, fetch)?;
                    let unit = field.map(|f| <[u8; UNITS_BYTES]>::try_from(&*f).map(i64::from_le_bytes)).transpose();
                    let unit = unit.map_err(|_| corrupt(&format!("column {at} is not a value of its type")))?;
                    units.push(unit.unwrap_or(0));
                    nulls.push(rows, unit.is_none());
                }
                Gathered::Texts { text, ends, pad, nulls } => {
                    let field = format.bytes_with(record, at, fetch)?;
                    if let Some(f) = &field {
                        let s = std::str::from_utf8(f).map_err(|_| corrupt(&format!("column {at} is not a value of its type")))?;
                        padded(text, s, *pad);
                    }
                    ends.push(text.len());
                    nulls.push(rows, field.is_none());
                }
                Gathered::Values(values) => {
                    let value = if at < format.columns() { format.column_with(record, at, fetch)? } else { Value::Null };
                    values.push(value);
                }
            }
        }
        self.rows = rows.saturating_add(1);
        Ok(())
    }

    /// The batch gathered, it then empty.
    pub(in crate::engine::rows) fn take(&mut self) -> Batch {
        let rows = std::mem::take(&mut self.rows);
        let columns = self
            .columns
            .iter_mut()
            .map(|(_, vector)| match vector {
                Gathered::Ints { units, scale, nulls } => Column::Ints(Ints::new(std::mem::take(units), *scale, nulls.0.take())),
                Gathered::Texts { text, ends, nulls, .. } => {
                    Column::Texts(Texts::new(std::mem::take(text), std::mem::take(ends), nulls.0.take()))
                }
                Gathered::Values(values) => Column::Values(std::mem::take(values)),
            })
            .collect();
        Batch { columns, rows }
    }
}

// Unit tests: records gathered as their rows decode, malformed fields
// refused (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
