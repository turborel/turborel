//! \[DESIGN\] RULES.md D31, D38: a spilled sort's runs, written as vectors.
//! Each record of a run is a chunk of its rows, in their order: their
//! count and width; each row's head, the tag and the prefix of its first
//! key (`merge.rs`), as two vectors; then each column, a byte of its kind,
//! a byte saying whether it has nulls and then, if it has, a byte a row,
//! and its vector: of exact numbers of one scale fitting 64 bits, the
//! scale and each row's units; of approximate numbers, each row's bits; of
//! character strings, each row's length, then their bytes. A column of
//! anything else, or of values of more than one kind, is written as each
//! row's value, as the value codec writes it (`codec::put_value`), with no
//! byte of nulls. Read back, a chunk is a batch of those vectors
//! (`Column::Ints`, `Floats`, `Texts` and `Values`), its rows compared and
//! made where they lie, as `vectors.rs` compares and makes them.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::storage::{Batch, Cell, Column, Floats, Ints, Texts};
use crate::value::Value;
use crate::value::codec::{put_value, value};
use crate::value::types::TypeRegistry;

/// A column's kind: each row's value.
const VALUES: u8 = 0;
/// A column's kind: exact numbers of one scale.
const INTS: u8 = 1;
/// A column's kind: approximate numbers.
const FLOATS: u8 = 2;
/// A column's kind: character strings.
const TEXTS: u8 = 3;

/// A row to write: a batch, and the row's place in it.
pub(super) type Pick<'a> = (&'a Batch, usize);

/// A chunk read back: each row's tag and prefix, and its vectors.
pub(super) struct Chunk {
    pub(super) tags: Vec<u8>,
    pub(super) prefixes: Vec<u64>,
    pub(super) batch: Batch,
}

/// Writes to `out`, emptied first, the chunk of `rows`, each row of the
/// width of the first's batch, beside its head of `heads`, its tag and
/// prefix.
pub(super) fn put_chunk(heads: &[(u8, u64)], rows: &[Pick<'_>], out: &mut Vec<u8>) {
    out.clear();
    let width = rows.first().map_or(0, |(batch, _)| batch.columns.len());
    out.put_u32(count(rows.len()));
    out.put_u32(count(width));
    for &(tag, _) in heads {
        out.put_u8(tag);
    }
    for &(_, bits) in heads {
        out.put_u64(bits);
    }
    for at in 0..width {
        put_column(rows, at, out);
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
        Cell::Value(Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_)) => Some((VALUES, 0_u32)),
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

/// Appends column `at` of `rows` to `out`, as the module's documentation
/// says.
fn put_column(rows: &[Pick<'_>], at: usize, out: &mut Vec<u8>) {
    let (kind, scale) = kind_of(rows, at);
    out.put_u8(kind);
    let cells = || rows.iter().map(move |&(batch, r)| batch.cell(at, r));
    if kind == VALUES {
        for cell in cells() {
            match cell {
                Cell::Value(v) => put_value(out, v),
                made @ (Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) | Cell::Null) => put_value(out, &made.to_value()),
            }
        }
        return;
    }
    if cells().any(|c| cell_kind(c).is_none()) {
        out.put_u8(1);
        for cell in cells() {
            out.put_u8(u8::from(cell_kind(cell).is_none()));
        }
    } else {
        out.put_u8(0);
    }
    if kind == INTS {
        out.put_u32(scale);
        cells().for_each(|c| out.put_u64(units(c).cast_unsigned()));
    } else if kind == FLOATS {
        cells().for_each(|c| out.put_u64(number(c).to_bits()));
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
        Cell::Value(_) | Cell::Approx(_) | Cell::Text(_) | Cell::Null => 0,
    }
}

/// A cell's approximate number; else 0.
fn number(cell: Cell<'_>) -> f64 {
    match cell {
        Cell::Approx(f) => f,
        Cell::Value(Value::Approx(f)) => *f,
        Cell::Value(_) | Cell::Exact(..) | Cell::Text(_) | Cell::Null => 0.0_f64,
    }
}

/// A cell's character string; else none.
fn text(cell: Cell<'_>) -> &str {
    match cell {
        Cell::Text(t) => t,
        Cell::Value(Value::Char(s)) => s,
        Cell::Value(_) | Cell::Exact(..) | Cell::Approx(_) | Cell::Null => "",
    }
}

/// The chunk of `record`, its values of types beyond the standard found
/// among `types`.
///
/// # Errors
///
/// SQLCODE -916 if it is not a chunk's record.
pub(super) fn chunk(record: &[u8], types: &TypeRegistry) -> Result<Chunk> {
    let mut reader = Reader::new(record, "sorted run").with_types(types);
    let rows = place(reader.u32()?);
    let width = place(reader.u32()?);
    let tags = reader.take(rows)?.to_vec();
    let prefixes = (0..rows).map(|_| reader.u64()).collect::<Result<Vec<u64>>>()?;
    let columns = (0..width).map(|_| column(&mut reader, rows)).collect::<Result<Vec<Column>>>()?;
    reader.finish()?;
    Ok(Chunk { tags, prefixes, batch: Batch { columns, rows } })
}

/// A count read, as a place.
fn place(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// A column of `rows` rows, read.
fn column(reader: &mut Reader<'_>, rows: usize) -> Result<Column> {
    let kind = reader.u8()?;
    if kind == VALUES {
        return Ok(Column::Values((0..rows).map(|_| value(reader)).collect::<Result<Vec<Value>>>()?));
    }
    let nulls = if reader.u8()? == 0 { None } else { Some(reader.take(rows)?.iter().map(|&b| b != 0).collect()) };
    if kind == INTS {
        let scale = reader.u32()?;
        let units = (0..rows).map(|_| reader.u64().map(u64::cast_signed)).collect::<Result<Vec<i64>>>()?;
        return Ok(Column::Ints(Ints::new(units, scale, nulls)));
    }
    if kind == FLOATS {
        let numbers = (0..rows).map(|_| reader.u64().map(f64::from_bits)).collect::<Result<Vec<f64>>>()?;
        return Ok(Column::Floats(Floats::new(numbers, nulls)));
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
    use crate::value::{Bits, Decimal};

    /// The rows of `batch` and their heads, written as a chunk and read
    /// back: the vectors' kinds, the heads, and the rows as strings.
    fn round_trip(batch: &Batch) -> (Vec<&'static str>, Vec<(u8, u64)>, Vec<String>) {
        let rows: Vec<Pick<'_>> = (0..batch.rows).map(|r| (batch, r)).collect();
        let heads: Vec<(u8, u64)> = (0..batch.rows).map(|r| (u8::try_from(r).unwrap(), u64::try_from(r).unwrap() << 40_u32)).collect();
        let mut out = Vec::new();
        put_chunk(&heads, &rows, &mut out);
        let read = chunk(&out, &TypeRegistry::default()).unwrap();
        let kinds = read
            .batch
            .columns
            .iter()
            .map(|c| match c {
                Column::Ints(_) => "ints",
                Column::Floats(_) => "floats",
                Column::Texts(_) => "texts",
                Column::Values(_) => "values",
                Column::Nulls(_) => "nulls",
            })
            .collect();
        let back: Vec<(u8, u64)> = read.tags.iter().copied().zip(read.prefixes.iter().copied()).collect();
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
    /// of other types, and all null, as values; each row's head kept.
    #[test]
    fn chunks_keep_their_vectors_kinds() {
        let exact = |n: i128, scale: u32| Value::Exact(Decimal::new(n, scale));
        let texts = Texts::new("abcde".to_owned(), vec![1_usize, 1_usize, 5_usize], Some(vec![false, true, false]));
        let coded = Texts::coded(
            crate::storage::Coded { codes: vec![1_u32, 0_u32, 1_u32], entries: Texts::new("xy".to_owned(), vec![1_usize, 2_usize], None) },
            None,
        );
        let batch = Batch {
            columns: vec![
                Column::Ints(Ints::new(vec![150_i64, 0_i64, -3_i64], 2_u32, Some(vec![false, true, false]))),
                Column::Values(vec![exact(7_i128, 1_u32), Value::Null, exact(-2_i128, 1_u32)]),
                Column::Floats(Floats::new(vec![1.5_f64, -0.0_f64, 2.0_f64], None)),
                Column::Texts(texts),
                Column::Texts(coded),
                Column::Values(vec![exact(1_i128, 0_u32), Value::Char("a".to_owned()), Value::Null]),
                Column::Values(vec![exact(i128::MAX, 0_u32), exact(1_i128, 0_u32), exact(2_i128, 0_u32)]),
                Column::Values(vec![exact(1_i128, 0_u32), exact(1_i128, 1_u32), Value::Null]),
                Column::Values(vec![Value::Bits(Bits::of_bits("101").unwrap()), Value::Null, Value::Null]),
                Column::nulls(3_usize),
            ],
            rows: 3,
        };
        let (kinds, heads, rows) = round_trip(&batch);
        assert_eq!(kinds, ["ints", "ints", "floats", "texts", "texts", "values", "values", "values", "values", "values"]);
        assert_eq!(heads, [(0_u8, 0_u64), (1_u8, 1_u64 << 40_u32), (2_u8, 2_u64 << 40_u32)]);
        let max = i128::MAX;
        assert_eq!(
            rows,
            [
                format!("1.50,0.7,1.5E0,a,y,1,{max},1,101,NULL"),
                "NULL,NULL,-0E0,NULL,x,a,1,0.1,NULL,NULL".to_owned(),
                "-0.03,-0.2,2E0,bcde,y,NULL,2,NULL,NULL,NULL".to_owned()
            ]
        );
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
        put_chunk(&[(0_u8, 0_u64)], &[(&batch, 0_usize)], &mut out);
        let refused = |record: &[u8]| chunk(record, &TypeRegistry::default()).err().map(|e| e.code);
        assert!(refused(&out).is_none());
        assert_eq!(refused(&out[..out.len() - 1]), Some(Code::Corrupt), "short");
        let mut longer = out.clone();
        longer.push(0_u8);
        assert_eq!(refused(&longer), Some(Code::Corrupt), "bytes past its vectors");
        // Its column's kind follows the count, width, tag and prefix.
        let kind = 4 + 4 + 1 + 8;
        let mut unknown = out.clone();
        unknown[kind] = 9_u8;
        assert_eq!(refused(&unknown), Some(Code::Corrupt), "no kind");
        let mut bad = out.clone();
        let last = bad.len() - 1;
        bad[last] = 0xff_u8;
        assert_eq!(refused(&bad), Some(Code::Corrupt), "not UTF-8");
    }
}
