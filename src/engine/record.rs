//! \[DESIGN\] The record format of the built-in engine (RULES.md D31): a
//! row's bytes, laid out by its columns' types, the same on every platform,
//! every field little-endian.
//!
//! ```text
//! record:  header u8 | null bitmap | fixed part | ends | variable bytes
//! header:  0, this format (a version's stamps come before the record,
//!          `rows/version.rs`)
//! bitmap:  a bit per column, set if NULL, column 0 the lowest of byte 0
//! fixed:   for each column whose type has a width in a record
//!          (`SqlType::record_width`), in column order, its bytes as its
//!          type writes them: an exact number's units at the column's
//!          scale (i64), an approximate number's IEEE-754 bits, a
//!          datetime's instant in UTC (i64, a zone i16 after it if zoned),
//!          an interval's units (i128), a BINARY string's octets; zeros for
//!          NULL
//! ends:    for each column of varying width, in column order, u32: where
//!          its bytes end, counted from the start of the variable bytes;
//!          the top bit set if the column is kept out of line
//! variable: each such column's bytes as its type writes them, a
//!          CHARACTER string's UTF-8 without trailing spaces, a VARCHAR's
//!          as it is; or, for a column kept out of line, a reference:
//!          where the value is (u64, the layout's to interpret), its length
//!          in bytes (u32), and its first bytes, up to 16, cut at a
//!          character's boundary
//! ```
//!
//! Every column is found from the format alone, without decoding the
//! others. A CHARACTER string is stored without its trailing spaces and
//! padded again on read [X3.135-1986 §8.7 GR7b], so short values in long
//! columns take only their own bytes.
//!
//! A long value is kept out of line, so that rows stay short and several
//! fit in a page (RULES.md D31): every value of varying width longer than
//! a threshold, and then the longest others until the record fits the most
//! a row may take. Where values out of line are kept is the layout's: it
//! stores and fetches them through closures given to `encode_spilling` and
//! the `_with` decoders. The reference holds a prefix of the value, which
//! no reader uses yet.

use std::borrow::Cow;

use super::fixed_width;
use crate::error::{Code, Result, SqlError};
use crate::storage::Row;
use crate::value::{DataType, Value};

mod image;
mod inline;
mod listed;
mod patched;

/// The record's header byte as this code writes it.
const HEADER: u8 = 0;
/// Bytes of the header, and of an end.
const HEADER_BYTES: usize = 1;
const END_BYTES: usize = 4;
/// The bit of an end marking its column as kept out of line.
const OUTSIDE: u32 = 1 << 31;
/// Bytes of a reference before its prefix: where, and how long.
const REFERENCE: usize = 12;
/// The most bytes of a value a reference repeats.
const PREFIX: usize = 16;
/// Bits in a byte of the null bitmap.
const BITS: usize = 8;

/// Of each column of a row, the bytes of its value of varying width, as
/// its kind encodes them, if it has one: measured, then written.
type Encoded = Vec<Option<Vec<u8>>>;

/// Where a column is in a record.
#[derive(Debug, Clone, Copy)]
enum Place {
    /// At this offset in the fixed part, of this width, as its type's kind
    /// says (`SqlType::record_width`).
    Fixed(usize, usize),
    /// The `n`-th column of varying width.
    Variable(usize),
}

/// \[DESIGN\] The layout of the records of one table's columns.
#[derive(Debug, Clone)]
pub struct RecordFormat {
    types: Vec<DataType>,
    places: Vec<Place>,
    /// Bytes of the null bitmap and of the fixed part, and columns of
    /// varying width, each with an end.
    bitmap: usize,
    fixed: usize,
    variable_columns: usize,
}

/// A value kept out of line, as its record refers to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reference {
    /// Its column.
    pub column: usize,
    /// Where the layout keeps it.
    pub at: u64,
    /// Its length in bytes.
    pub length: usize,
}

/// \[DESIGN\] When values go out of line (RULES.md D31).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spill {
    /// A value of varying width whose record bytes are longer than this
    /// goes out of line.
    pub threshold: usize,
    /// The most bytes a record may take: the longest values left go out of
    /// line until it fits.
    pub most: usize,
}

/// A column's field in a record.
enum Field<'a> {
    Null,
    /// Its bytes, in the record.
    Inline(&'a [u8]),
    Outside(Reference),
}

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a stored row is malformed: {why}"))
}

/// The fetch of a format with nothing out of line: refuses.
pub(in crate::engine) fn not_fetched(reference: Reference) -> Result<Vec<u8>> {
    Err(SqlError::new(Code::Api, format!("[DESIGN] column {} is kept out of line: read it through its layout", reference.column)))
}

impl RecordFormat {
    /// Its columns.
    pub fn columns(&self) -> usize {
        self.types.len()
    }

    /// Its columns' types, in order.
    pub fn types(&self) -> &[DataType] {
        &self.types
    }

    /// The format for columns of `types`, in order.
    pub fn new(types: &[DataType]) -> RecordFormat {
        let mut fixed = 0_usize;
        let mut variable = 0_usize;
        let places = types
            .iter()
            .map(|t| {
                if let Some(width) = fixed_width(t) {
                    let place = Place::Fixed(fixed, width);
                    fixed = fixed.saturating_add(width);
                    return place;
                }
                let place = Place::Variable(variable);
                variable = variable.saturating_add(1);
                place
            })
            .collect();
        RecordFormat { types: types.to_vec(), places, bitmap: types.len().div_ceil(BITS), fixed, variable_columns: variable }
    }

    /// Where the fixed part, the ends and the variable bytes begin.
    fn fixed_at(&self) -> usize {
        HEADER_BYTES.saturating_add(self.bitmap)
    }
    fn ends_at(&self) -> usize {
        self.fixed_at().saturating_add(self.fixed)
    }
    fn variable_at(&self) -> usize {
        self.ends_at().saturating_add(self.variable_columns.saturating_mul(END_BYTES))
    }

    /// Appends the record of `row` to `out`, every value in line.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `row` does not have a value of its column's type, or
    /// NULL, for every column.
    pub fn encode(&self, row: &Row, out: &mut Vec<u8>) -> Result<()> {
        self.encode_in_line(row, None, out).map(|_| ())
    }

    /// Appends the record of `row` to `out`, the values `spill` says kept
    /// out of line by `store`, which is given each one's column and bytes
    /// and returns where it keeps them.
    ///
    /// # Errors
    ///
    /// As `encode`; SQLCODE -900 if the record is longer than `spill.most`
    /// even with every long value out of line; `store`'s errors.
    pub fn encode_spilling(
        &self,
        row: &Row,
        spill: Spill,
        out: &mut Vec<u8>,
        mut store: impl FnMut(usize, &[u8]) -> Result<u64>,
    ) -> Result<()> {
        if self.encode_in_line(row, Some(spill), out)? {
            return Ok(());
        }
        let (outside, encoded) = self.choose(row, spill)?;
        self.encode_as((row, encoded), &outside, out, &mut store)
    }

    fn check_len(&self, row: &Row) -> Result<()> {
        if row.len() == self.types.len() {
            return Ok(());
        }
        Err(SqlError::new(Code::Api, format!("[DESIGN] a row of {} values for a table of {} columns", row.len(), self.types.len())))
    }

    /// Which columns of `row` go out of line: those longer than the
    /// threshold, then the longest others until the record fits; each
    /// value of varying width measured by the bytes its record holds, as
    /// its type writes them. Those bytes too, kept for the record, so no
    /// value is encoded twice.
    fn choose(&self, row: &Row, spill: Spill) -> Result<(Vec<bool>, Encoded)> {
        self.check_len(row)?;
        let mut encoded = Vec::with_capacity(row.len());
        for (value, (data_type, place)) in row.iter().zip(self.types.iter().zip(&self.places)) {
            encoded.push(match (place, value.is_null()) {
                (Place::Variable(_), false) => Some(encode_value(value, data_type)?),
                (Place::Variable(_), true) | (Place::Fixed(..), _) => None,
            });
        }
        let lengths: Vec<Option<usize>> = encoded.iter().map(|bytes| bytes.as_ref().map(Vec::len)).collect();
        let mut outside: Vec<bool> = lengths.iter().map(|l| l.is_some_and(|l| l > spill.threshold)).collect();
        let size = |marks: &[bool]| {
            lengths.iter().zip(marks).fold(self.variable_at(), |n, (length, &out)| {
                let bytes = length.map_or(0, |l| if out { REFERENCE.saturating_add(l.min(PREFIX)) } else { l });
                n.saturating_add(bytes)
            })
        };
        while size(&outside) > spill.most {
            // Only a value longer than its reference shortens the record.
            let longest = lengths
                .iter()
                .zip(&outside)
                .enumerate()
                .filter(|(_, (length, out))| !**out && length.is_some_and(|l| l > REFERENCE.saturating_add(PREFIX)))
                .max_by_key(|(_, (length, _))| **length)
                .map(|(i, _)| i);
            let Some(slot) = longest.and_then(|i| outside.get_mut(i)) else {
                return Err(SqlError::new(
                    Code::Unsupported,
                    format!(
                        "[DESIGN] a row of {} bytes is longer than the {} a row may take, even with its long values out of line",
                        size(&outside),
                        spill.most
                    ),
                ));
            };
            *slot = true;
        }
        Ok((outside, encoded))
    }

    /// Appends the record of `row`, the columns `outside` marks kept by
    /// `store`; of its values of varying width, those `encoded` holds
    /// taken as encoded already (`choose`). Written into `out` in place,
    /// the fixed part sized at once and the values of varying width after
    /// it, not built apart and copied; on failure `out` is as it was.
    fn encode_as(
        &self,
        (row, mut encoded): (&Row, Encoded),
        outside: &[bool],
        out: &mut Vec<u8>,
        store: &mut impl FnMut(usize, &[u8]) -> Result<u64>,
    ) -> Result<()> {
        self.check_len(row)?;
        let base = out.len();
        // Room for the whole record at once: its values of varying width
        // measured, if they were, so `out` does not grow as they are written.
        let measured = encoded.iter().flatten().fold(0_usize, |sum, bytes| sum.saturating_add(bytes.len()));
        out.reserve(self.variable_at().saturating_add(measured));
        out.resize(base.saturating_add(self.variable_at()), 0);
        if let Some(header) = out.get_mut(base) {
            *header = HEADER;
        }
        let mut written = Ok(());
        for (i, (value, (data_type, &place))) in row.iter().zip(self.types.iter().zip(&self.places)).enumerate() {
            let bytes = match encoded.get_mut(i).and_then(Option::take) {
                Some(bytes) => Ok(Some(bytes)),
                None => (!value.is_null()).then(|| encode_value(value, data_type)).transpose(),
            };
            let column = (i, place, outside.get(i).copied().unwrap_or_default());
            written = bytes.and_then(|bytes| self.put_column(column, bytes.as_deref(), (out, base), store));
            if written.is_err() {
                out.truncate(base);
                break;
            }
        }
        written
    }

    /// Writes column `i`'s `bytes`, its value's, at `place` of the record
    /// that begins at `base` of `out`, or its null bit for none; kept out
    /// of line by `store` if `outside`.
    fn put_column(
        &self,
        (i, place, outside): (usize, Place, bool),
        bytes: Option<&[u8]>,
        (out, base): (&mut Vec<u8>, usize),
        store: &mut impl FnMut(usize, &[u8]) -> Result<u64>,
    ) -> Result<()> {
        let mut flag = 0;
        match (bytes, place, outside) {
            (None, ..) => {
                let (at, mask) = null_bit(i);
                if let Some(byte) = out.get_mut(base.saturating_add(at)) {
                    *byte |= mask;
                }
            }
            (Some(bytes), Place::Fixed(at, _), _) => {
                put(out.get_mut(base..).unwrap_or_default(), self.fixed_at().saturating_add(at), bytes);
            }
            (Some(bytes), Place::Variable(_), false) => out.extend_from_slice(bytes),
            (Some(bytes), Place::Variable(_), true) => {
                out.extend_from_slice(&store(i, bytes)?.to_le_bytes());
                out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_le_bytes());
                out.extend_from_slice(prefix(bytes));
                flag = OUTSIDE;
            }
        }
        if let Place::Variable(n) = place {
            self.put_end((out, base), n, flag)?;
        }
        Ok(())
    }

    /// Writes the end of the `n`-th column of varying width, whose bytes
    /// were just written, of the record that begins at `base` of `out`,
    /// marked with `flag`.
    fn put_end(&self, (out, base): (&mut Vec<u8>, usize), n: usize, flag: u32) -> Result<()> {
        let variable = out.len().saturating_sub(base.saturating_add(self.variable_at()));
        let end = u32::try_from(variable)
            .ok()
            .filter(|&e| e < OUTSIDE)
            .ok_or_else(|| SqlError::new(Code::Unsupported, "[DESIGN] a row's values of varying width take 2 GiB or more".to_owned()))?;
        let at = self.ends_at().saturating_add(n.saturating_mul(END_BYTES));
        put(out.get_mut(base..).unwrap_or_default(), at, &(end | flag).to_le_bytes());
        Ok(())
    }

    /// The row a record holds, with nothing out of line.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `bytes` are not a record of this format; SQLCODE
    /// -910 if a value is out of line.
    pub fn decode(&self, bytes: &[u8]) -> Result<Row> {
        self.decode_with(bytes, &mut not_fetched)
    }

    /// The row a record holds, values out of line got by `fetch`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `bytes` are not a record of this format, or a value
    /// fetched is not as long as its reference says; `fetch`'s errors.
    pub fn decode_with(&self, bytes: &[u8], fetch: &mut impl FnMut(Reference) -> Result<Vec<u8>>) -> Result<Row> {
        self.decode_some(bytes, None, fetch)
    }

    /// The row a record holds, with only `columns` decoded: the others NULL,
    /// as `TableStorage::scan` allows (RULES.md D31).
    ///
    /// # Errors
    ///
    /// As `decode`.
    pub fn decode_columns(&self, bytes: &[u8], columns: &[usize]) -> Result<Row> {
        self.decode_columns_with(bytes, columns, &mut not_fetched)
    }

    /// As `decode_columns`, values out of line got by `fetch`: only those
    /// of `columns` are fetched.
    ///
    /// # Errors
    ///
    /// As `decode_with`.
    pub fn decode_columns_with(
        &self,
        bytes: &[u8],
        columns: &[usize],
        fetch: &mut impl FnMut(Reference) -> Result<Vec<u8>>,
    ) -> Result<Row> {
        self.decode_some(bytes, Some(columns), fetch)
    }

    /// The row a record holds, `columns` decoded, or all, the others NULL:
    /// sized once, as a row is decoded for every one read, and a vector
    /// collected through `Result` would grow from a few values by doubling.
    /// Each column of `columns` is decoded once, where it is named, and one
    /// that is not a column of the record is passed over (RULES.md D751).
    fn decode_some(&self, bytes: &[u8], columns: Option<&[usize]>, fetch: &mut impl FnMut(Reference) -> Result<Vec<u8>>) -> Result<Row> {
        self.check(bytes)?;
        let Some(columns) = columns else {
            let mut row = Vec::with_capacity(self.types.len());
            for i in 0..self.types.len() {
                row.push(self.value_in(bytes, i, fetch)?);
            }
            return Ok(row);
        };
        let mut row = vec![Value::Null; self.types.len()];
        for &i in columns {
            // Named again, a column already decoded is not; a null one is
            // found null again at once.
            if let Some(slot) = row.get_mut(i).filter(|slot| slot.is_null()) {
                *slot = self.value_in(bytes, i, fetch)?;
            }
        }
        Ok(row)
    }

    /// Column `i` of a record, decoding nothing else.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `bytes` are not a record of this format, or `i` is
    /// not one of its columns; SQLCODE -910 if the value is out of line.
    pub fn column(&self, bytes: &[u8], i: usize) -> Result<Value> {
        self.column_with(bytes, i, &mut not_fetched)
    }

    /// Column `i` of a record, got by `fetch` if it is out of line.
    ///
    /// # Errors
    ///
    /// As `decode_with`.
    pub fn column_with(&self, bytes: &[u8], i: usize, fetch: &mut impl FnMut(Reference) -> Result<Vec<u8>>) -> Result<Value> {
        let data_type = self.types.get(i).ok_or_else(|| corrupt(&format!("it has no column {i}")))?;
        let Some(field) = self.bytes_with(bytes, i, fetch)? else { return Ok(Value::Null) };
        decode_value(&field, data_type).ok_or_else(|| corrupt(&format!("column {i} is not a value of its type")))
    }

    /// The bytes of column `i` of a record as its type writes them, got by
    /// `fetch` if it is out of line; none if it is null.
    ///
    /// # Errors
    ///
    /// As `decode_with`.
    pub fn bytes_with<'a>(
        &self,
        bytes: &'a [u8],
        i: usize,
        fetch: &mut impl FnMut(Reference) -> Result<Vec<u8>>,
    ) -> Result<Option<Cow<'a, [u8]>>> {
        match self.field(bytes, i)? {
            Field::Null => Ok(None),
            Field::Inline(field) => Ok(Some(Cow::Borrowed(field))),
            Field::Outside(reference) => {
                let fetched = fetch(reference)?;
                if fetched.len() != reference.length {
                    return Err(corrupt(&format!("column {i}'s value out of line is not of the length its reference says")));
                }
                Ok(Some(Cow::Owned(fetched)))
            }
        }
    }

    /// Whether column `i` of a record is read from the record alone: null,
    /// or not kept out of line.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `bytes` are not a record of this format.
    pub fn inline(&self, bytes: &[u8], i: usize) -> Result<bool> {
        Ok(match self.field(bytes, i)? {
            Field::Null | Field::Inline(_) => true,
            Field::Outside(_) => false,
        })
    }

    /// The values a record keeps out of line.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `bytes` are not a record of this format.
    pub fn outside(&self, bytes: &[u8]) -> Result<Vec<Reference>> {
        let mut found = Vec::new();
        for (i, place) in self.places.iter().enumerate() {
            if let (Place::Variable(_), Field::Outside(reference)) = (place, self.field(bytes, i)?) {
                found.push(reference);
            }
        }
        Ok(found)
    }

    /// Column `i`'s field in a record.
    fn field<'a>(&self, bytes: &'a [u8], i: usize) -> Result<Field<'a>> {
        self.check(bytes)?;
        self.field_in(bytes, i)
    }

    /// Whether `bytes` are of this format and long enough for its fixed
    /// part and its ends: checked once for a record, not for each column.
    fn check(&self, bytes: &[u8]) -> Result<()> {
        if bytes.first() != Some(&HEADER) || bytes.len() < self.variable_at() {
            return Err(corrupt("it is too short or of another format"));
        }
        Ok(())
    }

    /// Column `i`'s field in a record already checked (`check`).
    fn field_in<'a>(&self, bytes: &'a [u8], i: usize) -> Result<Field<'a>> {
        let place = self.places.get(i).copied().ok_or_else(|| corrupt(&format!("it has no column {i}")))?;
        let (null_at, mask) = null_bit(i);
        if bytes.get(null_at).copied().unwrap_or_default() & mask != 0 {
            return Ok(Field::Null);
        }
        let outside = || corrupt(&format!("column {i} lies outside it"));
        match place {
            Place::Fixed(at, width) => {
                let from = self.fixed_at().saturating_add(at);
                bytes.get(from..from.saturating_add(width)).map(Field::Inline).ok_or_else(outside)
            }
            Place::Variable(n) => match self.variable_field(bytes, n).ok_or_else(outside)? {
                (field, false) => Ok(Field::Inline(field)),
                (field, true) => reference(field, i).map(Field::Outside),
            },
        }
    }

    /// The bytes of the `n`-th column of varying width of a record, and
    /// whether they are a reference to a value out of line.
    fn variable_field<'a>(&self, bytes: &'a [u8], n: usize) -> Option<(&'a [u8], bool)> {
        let end_of = |k: usize| -> Option<u32> {
            let at = self.ends_at().checked_add(k.checked_mul(END_BYTES)?)?;
            Some(u32::from_le_bytes(<[u8; END_BYTES]>::try_from(bytes.get(at..at.checked_add(END_BYTES)?)?).ok()?))
        };
        let offset = |end: u32| usize::try_from(end & !OUTSIDE).ok();
        let begin = if n == 0 { 0 } else { offset(end_of(n.checked_sub(1)?)?)? };
        let end = end_of(n)?;
        let base = self.variable_at();
        Some((bytes.get(base.checked_add(begin)?..base.checked_add(offset(end)?)?)?, end & OUTSIDE != 0))
    }
}

/// The reference in `field`, column `column`'s.
fn reference(field: &[u8], column: usize) -> Result<Reference> {
    let malformed = || corrupt(&format!("column {column}'s reference to its value out of line is malformed"));
    let at = field.get(..8).and_then(|b| <[u8; 8]>::try_from(b).ok()).map(u64::from_le_bytes).ok_or_else(malformed)?;
    let length = field.get(8..REFERENCE).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes).ok_or_else(malformed)?;
    let length = usize::try_from(length).map_err(|_| malformed())?;
    let prefix = field.len().saturating_sub(REFERENCE);
    if prefix > PREFIX || prefix > length {
        return Err(malformed());
    }
    Ok(Reference { column, at, length })
}

/// The first bytes of `bytes`, up to `PREFIX`, cut at a character's start:
/// a UTF-8 continuation byte is `10xxxxxx`.
fn prefix(bytes: &[u8]) -> &[u8] {
    let mut end = PREFIX.min(bytes.len());
    while end > 0 && bytes.get(end).is_some_and(|b| b & 0xC0 == 0x80) {
        end = end.saturating_sub(1);
    }
    bytes.get(..end).unwrap_or_default()
}

/// Where column `i`'s null bit is: the byte's offset in the record, and the
/// bit within it.
fn null_bit(i: usize) -> (usize, u8) {
    (HEADER_BYTES.saturating_add(i.div_euclid(BITS)), 1 << i.rem_euclid(BITS))
}

/// Copies `bytes` into `record` at `at`; the record was sized for it.
fn put(record: &mut [u8], at: usize, bytes: &[u8]) {
    if let Some(slot) = record.get_mut(at..at.saturating_add(bytes.len())) {
        slot.copy_from_slice(bytes);
    }
}

/// The bytes of non-null `value` in a column of `data_type`, as its kind
/// encodes them.
fn encode_value(value: &Value, data_type: &DataType) -> Result<Vec<u8>> {
    data_type.encode_record(value)
}

/// The value `field` holds in a column of `data_type`, if it is one.
fn decode_value(field: &[u8], data_type: &DataType) -> Option<Value> {
    data_type.decode_record(field)
}

// Unit tests: rows read back as written, any column alone, values out of
// line, and malformed records refused (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
