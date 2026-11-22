//! \[DESIGN\] A batch gathered from records (RULES.md D31,
//! `TableStorage::scan_batches`): each column asked for decoded straight
//! from each record's field into its vector, no row made: an exact
//! number's units, written as an integer of 8 bytes (`record.rs`), into
//! integers (`storage::Ints`); a character string's text into one buffer
//! (`storage::Texts`), a CHARACTER string padded to its length as it is
//! read [X3.135-1986 §8.7 GR7b]; any other value decoded as a row's is.
//! So the row store's batches, and the column store's rows not yet
//! frozen, are of the vectors its frozen rows give.

use super::corrupt;
use crate::engine::columns::{padded, padding};
use crate::engine::record::{RecordFormat, Reference};
use crate::error::Result;
use crate::storage::{Batch, Bools, Column, Datetimes, Ints, Mask, Texts};
use crate::value::datetime::Moment;
use crate::value::types::narrow_scale;
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
    /// Datetimes of a type, as their instants (`storage::Datetimes`).
    Datetimes(Box<(DataType, Datetimes)>, Nulls),
    /// Truth values, a bit each (`storage::Bools`).
    Bools(Mask, Nulls),
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
        if let Some(scale) = t.and_then(narrow_scale) {
            return Gathered::Ints { units: Vec::new(), scale, nulls: Nulls::default() };
        }
        match t {
            Some(DataType::Boolean) => Gathered::Bools(Mask::none(0), Nulls::default()),
            Some(t @ DataType::Date) => Self::datetimes(t, (Moment::Date, false, 0)),
            Some(t @ DataType::Time { precision, zoned }) => Self::datetimes(t, (Moment::Time, *zoned, *precision)),
            Some(t @ DataType::Timestamp { precision, zoned }) => Self::datetimes(t, (Moment::Timestamp, *zoned, *precision)),
            Some(other) => match padding(other) {
                Some(pad) => Gathered::Texts { text: String::new(), ends: Vec::new(), pad, nulls: Nulls::default() },
                None => Gathered::Values(Vec::new()),
            },
            None => Gathered::Values(Vec::new()),
        }
    }

    /// The vector a datetime column of type `t` is gathered into, of the
    /// fields, zone and precision `(moment, zoned, precision)`.
    fn datetimes(t: &DataType, (moment, zoned, precision): (Moment, bool, u32)) -> Gathered {
        let shape = (moment, zoned, u8::try_from(precision).unwrap_or(u8::MAX));
        let made = Datetimes::new(Vec::new(), zoned.then(Vec::new), shape, None);
        Gathered::Datetimes(Box::new((t.clone(), made)), Nulls::default())
    }

    /// How many rows it holds.
    pub(in crate::engine::rows) fn rows(&self) -> usize {
        self.rows
    }

    /// \[DESIGN\] Room in each column's vector for `rows` rows in all, so
    /// a batch grows to them at once, not doubling from none as each
    /// batch's vectors are handed on (RULES.md D822).
    pub(in crate::engine::rows) fn reserve(&mut self, rows: usize) {
        let more = rows.saturating_sub(self.rows);
        for (_, vector) in &mut self.columns {
            match vector {
                Gathered::Ints { units, .. } => units.reserve(more),
                Gathered::Texts { ends, .. } => ends.reserve(more),
                Gathered::Datetimes(held, _) => {
                    held.1.utc.reserve(more);
                    if let Some(zones) = &mut held.1.zones {
                        zones.reserve(more);
                    }
                }
                Gathered::Bools(..) => {}
                Gathered::Values(values) => values.reserve(more),
            }
        }
    }

    /// Whether it reads any column.
    pub(in crate::engine::rows) fn reads_any(&self) -> bool {
        !self.columns.is_empty()
    }

    /// Whether it reads a plain `record` of `format` from the record alone:
    /// of no column, or of columns each null or kept in line.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `record` is not a record of `format`.
    pub(in crate::engine::rows) fn inline(&self, format: &RecordFormat, record: &[u8]) -> Result<bool> {
        for &(at, _) in &self.columns {
            if at < format.columns() && !format.inline(record, at)? {
                return Ok(false);
            }
        }
        Ok(true)
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
        lobs: Option<&std::sync::Arc<super::lobs::LobSpace>>,
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
                Gathered::Datetimes(held, nulls) => {
                    let d = datetime_of(format.bytes_with(record, at, fetch)?.as_deref(), &held.0, at)?;
                    let made = &mut held.1;
                    made.utc.push(d.map_or(0, |d| d.utc));
                    if let Some(zones) = &mut made.zones {
                        zones.push(d.map_or(0, |d| d.zone));
                    }
                    nulls.push(rows, d.is_none());
                }
                Gathered::Bools(bits, nulls) => {
                    let b = bool_of(format.bytes_with(record, at, fetch)?.as_deref(), at)?;
                    bits.push(b.unwrap_or(false));
                    nulls.push(rows, b.is_none());
                }
                Gathered::Values(values) => {
                    let mut value = [if at < format.columns() { format.column_with(record, at, fetch)? } else { Value::Null }];
                    if let (Some(lobs), [v]) = (lobs, &value)
                        && v.as_lob().is_some()
                    {
                        lobs.resolve_column(at, &mut value);
                    }
                    let [value] = value;
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
                Gathered::Ints { units, scale, nulls } => {
                    Column::Ints(Ints::new(std::mem::take(units), *scale, nulls.0.take().map(Mask::from)))
                }
                Gathered::Texts { text, ends, nulls, .. } => {
                    Column::Texts(Texts::new(std::mem::take(text), std::mem::take(ends), nulls.0.take().map(Mask::from)))
                }
                Gathered::Datetimes(held, nulls) => {
                    let made = &mut held.1;
                    let shape = (made.moment, made.zoned, made.precision);
                    let zones = made.zones.as_mut().map(std::mem::take);
                    Column::Datetimes(Datetimes::new(std::mem::take(&mut made.utc), zones, shape, nulls.0.take().map(Mask::from)))
                }
                Gathered::Bools(bits, nulls) => Column::Bools(Bools::new(std::mem::take(bits), nulls.0.take().map(Mask::from))),
                Gathered::Values(values) => Column::Values(std::mem::take(values)),
            })
            .collect();
        Batch { columns, rows }
    }
}

// Unit tests: records gathered as their rows decode, malformed fields
// refused (RULES.md R-TEST-1, R-TEST-2).
/// The datetime of type `t` a record's `field` of column `at` holds, none
/// if null.
///
/// # Errors
///
/// SQLCODE -916 if it is not a value of the type.
fn datetime_of(field: Option<&[u8]>, t: &DataType, at: usize) -> Result<Option<crate::value::Datetime>> {
    let Some(f) = field else { return Ok(None) };
    match t.decode_record(f) {
        Some(Value::Datetime(d)) => Ok(Some(d)),
        Some(_) | None => Err(corrupt(&format!("column {at} is not a value of its type"))),
    }
}

/// The truth value a record's `field` of column `at` holds, none if null.
///
/// # Errors
///
/// SQLCODE -916 if it is not a truth value.
fn bool_of(field: Option<&[u8]>, at: usize) -> Result<Option<bool>> {
    let Some(f) = field else { return Ok(None) };
    match DataType::Boolean.decode_record(f) {
        Some(Value::Bool(b)) => Ok(Some(b)),
        Some(_) | None => Err(corrupt(&format!("column {at} is not a value of its type"))),
    }
}

#[cfg(test)]
mod tests;
