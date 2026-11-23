//! \[DESIGN\] RULES.md D31, D38, D364: a spilled sort's runs, written as
//! vectors. Each record of a run is a chunk of its rows, in their order:
//! their count and width; the layout their normal keys are of (0 if they
//! have none), whether rows of equal keys may differ, and the keys' width,
//! then each row's key (`normal.rs`); then each column, a byte of its kind,
//! a byte saying whether it has nulls and then, if it has, a bit a row,
//! least significant first (`Mask::lsb_bytes`), and its vector: of exact
//! numbers of one scale fitting 64 bits, the scale and each row's units; of
//! approximate numbers, each row's bits; of character strings, each row's
//! length, then their bytes; of datetimes of one type, the type, each
//! row's instant and its displacement; of truth values, a bit a row. A
//! column of anything else, or of values of more than one kind, is
//! written as each row's value, as the value codec writes it, what it
//! names after it (`SpilledLobs::put_value`), with no byte of nulls. Read
//! back, a chunk is a batch of those vectors (`Column::Ints`, `Floats`,
//! `Texts`, `Datetimes`, `Bools` and `Values`), its rows compared and made
//! where they lie, as `vectors.rs` compares and makes them.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::spill::SpilledLobs;
use crate::storage::mask::lsb_len;
use crate::storage::{Batch, Bools, Cell, Column, Datetimes, Floats, Ints, Mask, Texts};
use crate::value::datetime::Moment;
use crate::value::types::TypeRegistry;
use crate::value::{Datetime, Value};

/// A column's kind: each row's value.
const VALUES: u8 = 0;
/// A column's kind: exact numbers of one scale.
const INTS: u8 = 1;
/// A column's kind: approximate numbers.
const FLOATS: u8 = 2;
/// A column's kind: character strings.
const TEXTS: u8 = 3;
/// A column's kind: datetimes of one type.
const DATETIMES: u8 = 4;
/// A column's kind: truth values.
const BOOLS: u8 = 5;

/// A datetime type's code: its fields' in the low byte, its precision in
/// the next, and this bit if it is WITH TIME ZONE.
const ZONED: u32 = 1 << 16;

/// The bits of a byte, a datetime type's code shifted by them.
const BYTE: u32 = u8::BITS;

/// The fields of datetimes, in the order of their codes.
const MOMENTS: [Moment; 3] = [Moment::Date, Moment::Time, Moment::Timestamp];

/// A row to write: a batch, and the row's place in it.
pub(super) type Pick<'a> = (&'a Batch, usize);

/// The normal keys of a chunk's rows: the layout they are of, numbered
/// by the sort (0 for none), whether rows of equal keys may differ, each
/// key's width, and the keys, one after another.
#[derive(Clone, Copy, Default)]
pub(super) struct Keys<'k> {
    pub(super) layout: u32,
    pub(super) truncates: bool,
    pub(super) width: usize,
    pub(super) bytes: &'k [u8],
}

/// A chunk read back: its rows' keys, as `Keys` says, and its vectors.
pub(super) struct Chunk {
    pub(super) layout: u32,
    pub(super) truncates: bool,
    pub(super) width: usize,
    pub(super) keys: Vec<u8>,
    pub(super) batch: Batch,
}

impl Chunk {
    /// The key of its row `at`.
    pub(super) fn key(&self, at: usize) -> &[u8] {
        let from = at.saturating_mul(self.width);
        self.keys.get(from..from.saturating_add(self.width)).unwrap_or_default()
    }
}

/// Writes to `out`, emptied first, the chunk of `rows`, each row of the
/// width of the first's batch, their keys `keys`, naming what `lobs` holds.
pub(super) fn put_chunk(keys: Keys<'_>, rows: &[Pick<'_>], out: &mut Vec<u8>, lobs: &SpilledLobs) {
    out.clear();
    let width = rows.first().map_or(0, |(batch, _)| batch.columns.len());
    out.put_u32(count(rows.len()));
    out.put_u32(count(width));
    out.put_u32(keys.layout);
    out.put_u8(u8::from(keys.truncates));
    out.put_u32(count(keys.width));
    out.extend_from_slice(keys.bytes);
    for at in 0..width {
        put_column(rows, at, out, lobs);
    }
}

/// `n`, as a chunk counts it.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The kind a cell's value is written as, and its scale, of an exact
/// number's: none of a null.
fn cell_kind(cell: Cell<'_>) -> Option<(u8, u32)> {
    match cell {
        Cell::Null | Cell::Value(Value::Null) => None,
        Cell::Exact(_, scale) => Some((INTS, scale)),
        Cell::Value(Value::Exact(d)) => Some(i64::try_from(d.units).map_or((VALUES, 0_u32), |_| (INTS, d.scale))),
        Cell::Approx(_) | Cell::Value(Value::Approx(_)) => Some((FLOATS, 0_u32)),
        Cell::Text(_) | Cell::Value(Value::Char(_)) => Some((TEXTS, 0_u32)),
        Cell::Datetime(d) | Cell::Value(&Value::Datetime(d)) => Some((DATETIMES, type_code(&d))),
        Cell::Bool(_) | Cell::Value(Value::Bool(_)) => Some((BOOLS, 0_u32)),
        Cell::Value(
            Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_),
        ) => Some((VALUES, 0_u32)),
    }
}

/// The kind column `at` of `rows` is written as, and its scale: the one
/// kind of its values not null, if they are of one; else, or if all are
/// null, values.
fn kind_of(rows: &[Pick<'_>], at: usize) -> (u8, u32) {
    let mut kind = None;
    for &(batch, r) in rows {
        let Some(this) = cell_kind(batch.cell(at, r)) else { continue };
        if kind.is_some_and(|k| k != this) {
            return (VALUES, 0_u32);
        }
        kind = Some(this);
    }
    kind.unwrap_or((VALUES, 0_u32))
}

/// Appends column `at` of `rows` to `out`, naming what `lobs` holds, as
/// the module's documentation says.
fn put_column(rows: &[Pick<'_>], at: usize, out: &mut Vec<u8>, lobs: &SpilledLobs) {
    let (kind, scale) = kind_of(rows, at);
    out.put_u8(kind);
    let cells = || rows.iter().map(move |&(batch, r)| batch.cell(at, r));
    if kind == VALUES {
        for cell in cells() {
            match cell {
                Cell::Value(v) => lobs.put_value(out, v),
                made @ (Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) | Cell::Datetime(_) | Cell::Bool(_) | Cell::Null) => {
                    lobs.put_value(out, &made.to_value());
                }
            }
        }
        return;
    }
    if cells().any(|c| cell_kind(c).is_none()) {
        out.put_u8(1);
        out.extend(Mask::from_bools(&cells().map(|c| cell_kind(c).is_none()).collect::<Vec<bool>>()).lsb_bytes());
    } else {
        out.put_u8(0);
    }
    if kind == INTS {
        out.put_u32(scale);
        cells().for_each(|c| out.put_u64(units(c).cast_unsigned()));
    } else if kind == FLOATS {
        cells().for_each(|c| out.put_u64(number(c).to_bits()));
    } else if kind == DATETIMES {
        out.put_u32(scale);
        let instants: Vec<(i64, i16)> = cells().map(|c| instant(c).map_or((0, 0), |d| (d.utc, d.zone))).collect();
        for &(utc, _) in &instants {
            out.put_u64(utc.cast_unsigned());
        }
        for &(_, zone) in &instants {
            out.put_u32(u32::from(zone.cast_unsigned()));
        }
    } else if kind == BOOLS {
        out.extend(
            Mask::from_bools(&cells().map(|c| matches!(c, Cell::Bool(true) | Cell::Value(Value::Bool(true)))).collect::<Vec<bool>>())
                .lsb_bytes(),
        );
    } else {
        cells().for_each(|c| out.put_u32(count(text(c).len())));
        cells().for_each(|c| out.extend_from_slice(text(c).as_bytes()));
    }
}

/// A cell's units, of an exact number that fits 64 bits; else 0.
fn units(cell: Cell<'_>) -> i64 {
    match cell {
        Cell::Exact(units, _) => units,
        Cell::Value(Value::Exact(d)) => i64::try_from(d.units).unwrap_or(0),
        Cell::Value(_) | Cell::Approx(_) | Cell::Text(_) | Cell::Datetime(_) | Cell::Bool(_) | Cell::Null => 0,
    }
}

/// A cell's datetime; else none.
fn instant(cell: Cell<'_>) -> Option<Datetime> {
    match cell {
        Cell::Datetime(d) | Cell::Value(&Value::Datetime(d)) => Some(d),
        Cell::Value(_) | Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) | Cell::Bool(_) | Cell::Null => None,
    }
}

/// The code of `d`'s type, as `ZONED` says.
fn type_code(d: &Datetime) -> u32 {
    let moment = MOMENTS.iter().position(|&m| m == d.moment).and_then(|m| u32::try_from(m).ok()).unwrap_or(0);
    let zoned = if d.zoned { ZONED } else { 0 };
    moment | u32::from(d.precision).checked_shl(BYTE).unwrap_or(0) | zoned
}

/// The type of datetimes of `code`, as `Datetimes::new` takes it.
fn type_of(code: u32) -> Option<(Moment, bool, u8)> {
    let moment = MOMENTS.get(usize::try_from(code & u32::from(u8::MAX)).ok()?).copied()?;
    let precision = u8::try_from(code.checked_shr(BYTE)? & u32::from(u8::MAX)).ok()?;
    Some((moment, code & ZONED != 0, precision))
}

/// A cell's approximate number; else 0.
fn number(cell: Cell<'_>) -> f64 {
    match cell {
        Cell::Approx(f) => f,
        Cell::Value(Value::Approx(f)) => *f,
        Cell::Value(_) | Cell::Exact(..) | Cell::Text(_) | Cell::Datetime(_) | Cell::Bool(_) | Cell::Null => 0.0_f64,
    }
}

/// A cell's character string; else none.
fn text(cell: Cell<'_>) -> &str {
    match cell {
        Cell::Text(t) => t,
        Cell::Value(Value::Char(s)) => s,
        Cell::Value(_) | Cell::Exact(..) | Cell::Approx(_) | Cell::Datetime(_) | Cell::Bool(_) | Cell::Null => "",
    }
}

/// The chunk of `record`, its values of types beyond the standard found
/// among `types`, and what it names among `lobs`.
///
/// # Errors
///
/// SQLCODE -916 if it is not a chunk's record.
pub(super) fn chunk(record: &[u8], types: &TypeRegistry, lobs: &SpilledLobs) -> Result<Chunk> {
    let mut reader = Reader::new(record, "sorted run").with_types(types);
    let rows = place(reader.u32()?);
    let width = place(reader.u32()?);
    let layout = reader.u32()?;
    let truncates = reader.u8()? != 0;
    let key_width = place(reader.u32()?);
    let keys = reader.take(rows.saturating_mul(key_width))?.to_vec();
    let columns = (0..width).map(|_| column(&mut reader, rows, lobs)).collect::<Result<Vec<Column>>>()?;
    reader.finish()?;
    Ok(Chunk { layout, truncates, width: key_width, keys, batch: Batch { columns, rows } })
}

/// A count read, as a place.
fn place(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// A column of `rows` rows, read.
fn column(reader: &mut Reader<'_>, rows: usize, lobs: &SpilledLobs) -> Result<Column> {
    let kind = reader.u8()?;
    if kind == VALUES {
        return Ok(Column::Values((0..rows).map(|_| lobs.value(reader)).collect::<Result<Vec<Value>>>()?));
    }
    let nulls = if reader.u8()? == 0 { None } else { Some(Mask::from_lsb_bytes(reader.take(lsb_len(rows))?, 0, rows)) };
    if kind == INTS {
        let scale = reader.u32()?;
        let units = (0..rows).map(|_| reader.u64().map(u64::cast_signed)).collect::<Result<Vec<i64>>>()?;
        return Ok(Column::Ints(Ints::new(units, scale, nulls)));
    }
    if kind == FLOATS {
        let numbers = (0..rows).map(|_| reader.u64().map(f64::from_bits)).collect::<Result<Vec<f64>>>()?;
        return Ok(Column::Floats(Floats::new(numbers, nulls)));
    }
    if kind == DATETIMES {
        let code = reader.u32()?;
        let shape = type_of(code).ok_or_else(|| reader.corrupt(&format!("no datetime type of a sorted run has code {code}")))?;
        let utc = (0..rows).map(|_| reader.u64().map(u64::cast_signed)).collect::<Result<Vec<i64>>>()?;
        let zones = (0..rows).map(|_| reader.u32().map(|z| u16::try_from(z).unwrap_or(0).cast_signed())).collect::<Result<Vec<i16>>>()?;
        return Ok(Column::Datetimes(Datetimes::new(utc, Some(zones), shape, nulls)));
    }
    if kind == BOOLS {
        return Ok(Column::Bools(Bools::new(Mask::from_lsb_bytes(reader.take(lsb_len(rows))?, 0, rows), nulls)));
    }
    if kind != TEXTS {
        return Err(reader.corrupt(&format!("no vector of a sorted run has kind {kind}")));
    }
    let mut ends = Vec::new();
    let mut end = 0_usize;
    for _ in 0..rows {
        end = end.saturating_add(place(reader.u32()?));
        ends.push(end);
    }
    let bytes = reader.take(end)?.to_vec();
    let text = String::from_utf8(bytes).map_err(|_| reader.corrupt("a sorted run's text is not UTF-8"))?;
    Ok(Column::Texts(Texts::new(text, ends, nulls)))
}

// Unit tests: chunks of every kind of vector written and read back, and
// records that are not chunks (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use crate::value::{Binary, Decimal};

    /// The rows of `batch` and their heads, written as a chunk and read
    /// back: the vectors' kinds, the heads, and the rows as strings.
    fn round_trip(batch: &Batch) -> (Vec<&'static str>, Vec<Vec<u8>>, Vec<String>) {
        let rows: Vec<Pick<'_>> = (0..batch.rows).map(|r| (batch, r)).collect();
        let bytes: Vec<u8> = (0..batch.rows).flat_map(|r| [u8::try_from(r).unwrap(), 7_u8]).collect();
        let mut out = Vec::new();
        put_chunk(Keys { layout: 3, truncates: true, width: 2, bytes: &bytes }, &rows, &mut out, &SpilledLobs::new());
        let read = chunk(&out, &TypeRegistry::default(), &SpilledLobs::new()).unwrap();
        let kinds = read
            .batch
            .columns
            .iter()
            .map(|c| match c {
                Column::Ints(_) => "ints",
                Column::Floats(_) => "floats",
                Column::Datetimes(_) => "datetimes",
                Column::Bools(_) => "bools",
                Column::Texts(_) => "texts",
                Column::Values(_) => "values",
                Column::Nulls(_) => "nulls",
                Column::Late(_) => "late",
            })
            .collect();
        assert!(read.batch.rows == 0 || (read.layout, read.truncates, read.width) == (3, true, 2));
        let back: Vec<Vec<u8>> = (0..read.batch.rows).map(|r| read.key(r).to_vec()).collect();
        let shown = (0..read.batch.rows)
            .map(|r| (0..read.batch.columns.len()).map(|at| read.batch.cell(at, r).to_value().to_string()).collect::<Vec<_>>().join(","))
            .collect();
        (kinds, back, shown)
    }

    /// Verifies: [DESIGN] RULES.md D31, D38: a chunk's vectors are written
    /// by their values' kind and read back as vectors of it, their nulls
    /// kept: exact numbers of one scale as integers, of a vector of values
    /// too; approximate numbers; strings, of a store's dictionary too;
    /// values of two kinds, of exact numbers past 64 bits or of two scales,
    /// of other types, and all null, as values; datetimes of a type and
    /// their zones; truth values; each row's key kept.
    #[test]
    fn chunks_keep_their_vectors_kinds() {
        let exact = |n: i128, scale: u32| Value::Exact(Decimal::new(n, scale));
        let texts = Texts::new("abcde".to_owned(), vec![1_usize, 1_usize, 5_usize], Some(vec![false, true, false].into()));
        let coded = Texts::coded(
            crate::storage::Coded {
                codes: vec![1_u32, 0_u32, 1_u32],
                entries: std::sync::Arc::new(Texts::new("xy".to_owned(), vec![1_usize, 2_usize], None)),
            },
            None,
        );
        let batch = Batch {
            columns: vec![
                Column::Ints(Ints::new(vec![150_i64, 0_i64, -3_i64], 2_u32, Some(vec![false, true, false].into()))),
                Column::Values(vec![exact(7_i128, 1_u32), Value::Null, exact(-2_i128, 1_u32)]),
                Column::Floats(Floats::new(vec![1.5_f64, -0.0_f64, 2.0_f64], None)),
                Column::Texts(texts),
                Column::Texts(coded),
                Column::Values(vec![exact(1_i128, 0_u32), Value::Char("a".into()), Value::Null]),
                Column::Values(vec![exact(i128::MAX, 0_u32), exact(1_i128, 0_u32), exact(2_i128, 0_u32)]),
                Column::Values(vec![exact(1_i128, 0_u32), exact(1_i128, 1_u32), Value::Null]),
                Column::Values(vec![Value::Binary(Binary::new(vec![0xA0])), Value::Null, Value::Null]),
                Column::nulls(3_usize),
                Column::Datetimes(Datetimes::new(
                    vec![86_400_000_000_i64, 0_i64, -60_000_000_i64],
                    Some(vec![60_i16, 0_i16, -90_i16]),
                    (Moment::Timestamp, true, 3_u8),
                    Some(vec![false, true, false].into()),
                )),
                Column::Values(vec![Value::Bool(true), Value::Null, Value::Bool(false)]),
            ],
            rows: 3,
        };
        let (kinds, heads, rows) = round_trip(&batch);
        assert_eq!(
            kinds,
            ["ints", "ints", "floats", "texts", "texts", "values", "values", "values", "values", "values", "datetimes", "bools"]
        );
        assert_eq!(heads, [vec![0_u8, 7_u8], vec![1_u8, 7_u8], vec![2_u8, 7_u8]]);
        let made: Vec<String> = (0..3)
            .map(|r| (0..batch.columns.len()).map(|at| batch.cell(at, r).to_value().to_string()).collect::<Vec<_>>().join(","))
            .collect();
        assert_eq!(rows, made, "every value as written");
        let max = i128::MAX;
        assert!(rows[0].starts_with(&format!("1.50,0.7,1.5E0,a,y,1,{max},1,A0,NULL,")), "{rows:?}");
        let (none, _, empty) = round_trip(&Batch { columns: Vec::new(), rows: 0 });
        assert!(none.is_empty() && empty.is_empty());
    }

    /// Verifies: [DESIGN] RULES.md D38: a record that is not a chunk is
    /// refused with SQLCODE -916: cut short, of a vector of no kind, of
    /// text not UTF-8, or with bytes past its last vector.
    #[test]
    fn records_not_chunks_are_refused() {
        let batch = Batch { columns: vec![Column::Texts(Texts::new("ab".to_owned(), vec![2_usize], None))], rows: 1 };
        let mut out = Vec::new();
        put_chunk(Keys { layout: 1, truncates: false, width: 1, bytes: &[9_u8] }, &[(&batch, 0_usize)], &mut out, &SpilledLobs::new());
        let refused = |record: &[u8]| chunk(record, &TypeRegistry::default(), &SpilledLobs::new()).err().map(|e| e.code);
        assert!(refused(&out).is_none());
        assert_eq!(refused(&out[..out.len() - 1]), Some(Code::Corrupt), "short");
        let mut longer = out.clone();
        longer.push(0_u8);
        assert_eq!(refused(&longer), Some(Code::Corrupt), "bytes past its vectors");
        // Its column's kind follows the count, width, layout, truncation,
        // key width and key.
        let kind = 4 + 4 + 4 + 1 + 4 + 1;
        let mut unknown = out.clone();
        unknown[kind] = 9_u8;
        assert_eq!(refused(&unknown), Some(Code::Corrupt), "no kind");
        let mut bad = out.clone();
        let last = bad.len() - 1;
        bad[last] = 0xff_u8;
        assert_eq!(refused(&bad), Some(Code::Corrupt), "not UTF-8");
    }
}
