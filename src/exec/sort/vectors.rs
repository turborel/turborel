//! \[DESIGN\] RULES.md D31, D38: a sort of a stream's batches, each holding
//! every column of its rows (`batch::whole_width`), its rows made only as
//! they are given or written. The batches are held as they come, counted
//! by the bytes their vectors hold, and beside them each row given, its
//! batch and place there, and the prefix of its first key made of its
//! vector (`prefix::prefix_of_cell`). The rows are sorted as `prefix.rs`
//! sorts them: by prefix, ties compared by the keys' values, read of the
//! vectors; by the keys alone if the first key's values have no prefixes
//! or are of kinds not alike; stably, so rows of equal keys keep the
//! order they came in. While the batches fit the work area, each row is
//! made of the vectors as it is given; past it, each work area's worth is
//! written as a run, each record made of the vectors as `merge::record`
//! makes it of a row, and the runs merged as `sort.rs` merges them.

use std::cmp::Ordering;

use super::merge::put_head;
use super::prefix::{Kind, joined, prefix_of_cell};
use super::{Work, merged};
use crate::bytes::Put;
use crate::db::State;
use crate::error::Result;
use crate::exec::batch::Batches;
use crate::exec::{RowIter, broken, failed};
use crate::logical::SortKey;
use crate::spill::{SpillRun, heap_bytes};
use crate::storage::{Batch, Cell, Column, Row};
use crate::txn::TxId;
use crate::value::codec::put_value;
use crate::value::{Value, sort_cmp};

/// [X3.135-1986 §8.3 GR3b] The rows of `input`, a stream's batches each
/// holding every column of its rows, sorted by `keys`, in a work area of
/// `db`, for the statement of `tx`. Never inlined into the caller, whose
/// frame is on the stack while its input is made (RULES.md R-CF-6).
#[inline(never)]
pub(in crate::exec) fn sort_batches(keys: &[SortKey], input: Batches, db: &State, tx: TxId) -> RowIter {
    match sorted(keys, input, &mut Work::of(db, tx)) {
        Ok(rows) => rows,
        Err(e) => failed(e),
    }
}

/// `sort_batches`'s work.
fn sorted(keys: &[SortKey], input: Batches, work: &mut Work) -> Result<RowIter> {
    let (mut held, mut runs) = (Held::default(), Vec::new());
    for kept in input {
        let (batch, rows) = kept?;
        let rows = rows.unwrap_or_else(|| (0..batch.rows).collect());
        if let Some(k) = keys.iter().find(|k| k.column >= batch.columns.len()).filter(|_| !rows.is_empty()) {
            return Err(broken(&format!("sort column {} is not in the rows sorted", k.column)));
        }
        held.add(keys, batch, &rows);
        if held.bytes > work.limit() {
            runs.push(std::mem::take(&mut held).written(keys, work)?);
        }
    }
    if runs.is_empty() {
        return Ok(held.into_rows(keys));
    }
    if !held.places.is_empty() {
        runs.push(held.written(keys, work)?);
    }
    merged(keys, runs, work)
}

/// A row held: its batch's place among those held, its place in the
/// batch, and the prefix of its first key.
#[derive(Clone, Copy)]
struct Place {
    batch: usize,
    row: usize,
    prefix: u64,
}

/// Batches held, and their rows given, in the order they came.
struct Held {
    batches: Vec<Batch>,
    places: Vec<Place>,
    /// The kind of the first key's values so far, if they are alike and
    /// each has a prefix.
    kind: Option<Kind>,
    /// Whether every prefix holds its value whole.
    whole: bool,
    /// The bytes the batches and places hold.
    bytes: usize,
}

impl Default for Held {
    fn default() -> Held {
        Held { batches: Vec::new(), places: Vec::new(), kind: Some(Kind::Null), whole: true, bytes: 0 }
    }
}

impl Held {
    /// Holds `batch`, its rows `rows` given, sorted by `keys`.
    fn add(&mut self, keys: &[SortKey], batch: Batch, rows: &[usize]) {
        let at = self.batches.len();
        for &row in rows {
            let prefix = keys.first().and_then(|k| prefix_of_cell(batch.cell(k.column, row), k.descending));
            self.kind = self.kind.zip(prefix).and_then(|(kind, p)| joined(kind, p.kind));
            self.whole &= prefix.is_some_and(|p| p.whole);
            self.places.push(Place { batch: at, row, prefix: prefix.map_or(0, |p| p.bits) });
        }
        let places = rows.len().saturating_mul(size_of::<Place>());
        self.bytes = self.bytes.saturating_add(batch_bytes(&batch)).saturating_add(places);
        self.batches.push(batch);
    }

    /// Its places, sorted by `keys` [X3.135-1986 §8.3 GR3b], stably: by
    /// prefix, if the first key's values have them, ties by the keys'
    /// values; else by the values alone.
    fn sort(&mut self, keys: &[SortKey]) {
        let batches = &self.batches;
        let compare = |ties: &[SortKey], a: &Place, b: &Place| {
            batches.get(a.batch).zip(batches.get(b.batch)).map_or(Ordering::Equal, |(x, y)| compare_cells(ties, (x, a.row), (y, b.row)))
        };
        if self.kind.is_none() || keys.is_empty() {
            self.places.sort_by(|a, b| compare(keys, a, b));
            return;
        }
        // The keys rows of equal prefixes are compared by.
        let ties = if self.whole { keys.get(1..).unwrap_or_default() } else { keys };
        self.places.sort_by(|a, b| a.prefix.cmp(&b.prefix).then_with(|| compare(ties, a, b)));
    }

    /// Its rows, sorted by `keys`, each made as it is given.
    fn into_rows(mut self, keys: &[SortKey]) -> RowIter {
        self.sort(keys);
        let Held { batches, places, .. } = self;
        Box::new(places.into_iter().filter_map(move |p| batches.get(p.batch).map(|batch| Ok(row_of(batch, p.row)))))
    }

    /// Its rows, sorted by `keys`, written as a run of `work`'s spill area,
    /// each record made of the vectors as `merge::record` makes it of a row.
    fn written(mut self, keys: &[SortKey], work: &mut Work) -> Result<Box<dyn SpillRun>> {
        self.sort(keys);
        let (mut run, mut out) = (work.run()?, Vec::new());
        for p in &self.places {
            let Some(batch) = self.batches.get(p.batch) else { continue };
            put_head(keys.first().and_then(|k| prefix_of_cell(batch.cell(k.column, p.row), k.descending)), &mut out);
            put_cells(batch, p.row, &mut out);
            run.push(&out)?;
        }
        Ok(run)
    }
}

/// Row `r` of `batch`, made of its values.
fn row_of(batch: &Batch, r: usize) -> Row {
    (0..batch.columns.len()).map(|at| batch.cell(at, r).to_value()).collect()
}

/// Appends row `r` of `batch` to `out`, as `codec::put_row` writes the row
/// of its values, none made of a vector of values.
fn put_cells(batch: &Batch, r: usize, out: &mut Vec<u8>) {
    out.put_u32(u32::try_from(batch.columns.len()).unwrap_or(u32::MAX));
    for at in 0..batch.columns.len() {
        match batch.cell(at, r) {
            Cell::Value(v) => put_value(out, v),
            made @ (Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) | Cell::Null) => put_value(out, &made.to_value()),
        }
    }
}

/// [X3.135-1986 §8.3 GR3b] The order of row `ra` of `a` and row `rb` of
/// `b` under `keys`, as `compare_rows` orders the rows of their values.
fn compare_cells(keys: &[SortKey], (a, ra): (&Batch, usize), (b, rb): (&Batch, usize)) -> Ordering {
    keys.iter()
        .map(|k| {
            let o = cell_cmp(a.cell(k.column, ra), b.cell(k.column, rb));
            if k.descending { o.reverse() } else { o }
        })
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}

/// The order of the values of cells `x` and `y`, as `sort_cmp` orders the
/// values: integers of one scale by their units, values as they are read,
/// others made.
fn cell_cmp(x: Cell<'_>, y: Cell<'_>) -> Ordering {
    match (x, y) {
        (Cell::Exact(units, scale), Cell::Exact(other, other_scale)) => {
            if scale == other_scale {
                units.cmp(&other)
            } else {
                sort_cmp(&x.to_value(), &y.to_value())
            }
        }
        (Cell::Value(v), Cell::Value(w)) => sort_cmp(v, w),
        (Cell::Exact(..) | Cell::Value(_) | Cell::Approx(_) | Cell::Text(_) | Cell::Null, _) => sort_cmp(&x.to_value(), &y.to_value()),
    }
}

/// The bytes `batch`'s vectors hold, near enough to bound a work area by.
fn batch_bytes(batch: &Batch) -> usize {
    batch.columns.iter().fold(0_usize, |sum, column| sum.saturating_add(column_bytes(column)))
}

/// The bytes the vector `column` holds.
fn column_bytes(column: &Column) -> usize {
    let nulls = |nulls: &Option<Vec<bool>>| nulls.as_ref().map_or(0, Vec::len);
    match column {
        Column::Values(values) => {
            values.iter().fold(values.len().saturating_mul(size_of::<Value>()), |sum, v| sum.saturating_add(heap_bytes(v)))
        }
        Column::Ints(ints) => ints.units.len().saturating_mul(size_of::<i64>()).saturating_add(nulls(&ints.nulls)),
        Column::Floats(floats) => floats.numbers.len().saturating_mul(size_of::<f64>()).saturating_add(nulls(&floats.nulls)),
        Column::Texts(texts) => {
            let coded = texts.coded.as_ref().map_or(0, |c| {
                let entries = c.entries.text.len().saturating_add(c.entries.ends.len().saturating_mul(size_of::<usize>()));
                c.codes.len().saturating_mul(size_of::<u32>()).saturating_add(entries)
            });
            let ends = texts.ends.len().saturating_mul(size_of::<usize>());
            texts.text.len().saturating_add(ends).saturating_add(nulls(&texts.nulls)).saturating_add(coded)
        }
    }
}

// Unit tests: sorts of batches whose rows the integration tests do not
// make, and what a batch is counted as holding (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MemoryCatalog;
    use crate::error::Code;
    use crate::storage::{Coded, Floats, Ints, Texts};
    use crate::txn::NO_TRANSACTION;
    use crate::value::Decimal;

    /// The rows of `columns`, a batch of `rows` rows, sorted by `keys`, as
    /// strings, or the error's code.
    fn sorted_of(keys: &[SortKey], columns: Vec<Column>, rows: usize) -> std::result::Result<Vec<String>, Code> {
        let st = State::with_builtins(Box::new(MemoryCatalog::default()));
        let input: Batches = Box::new(std::iter::once(Ok((Batch { columns, rows }, None))));
        let made = sort_batches(keys, input, &st, NO_TRANSACTION).collect::<Result<Vec<Row>>>().map_err(|e| e.code)?;
        Ok(made.iter().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")).collect())
    }

    /// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §8.3 GR3b]: a key whose
    /// values are not all of one kind sorts the rows by their values alone,
    /// as `compare_rows` does, stably; exact numbers of two scales compare
    /// by value; a key not among a batch's columns is refused with SQLCODE
    /// -910.
    #[test]
    fn rows_of_kinds_not_alike_sort_by_their_values() {
        let int = |n: i128, scale: u32| Value::Exact(Decimal::new(n, scale));
        let mixed = Column::Values(vec![int(2_i128, 0_u32), Value::Char("a".to_owned()), int(15_i128, 1_u32), int(1_i128, 0_u32)]);
        let place = Column::Ints(Ints::new(vec![0_i64, 1_i64, 2_i64, 3_i64], 0_u32, None));
        let up = [SortKey { column: 0, descending: false }];
        let made = sorted_of(&up, vec![mixed.clone(), place.clone()], 4_usize).unwrap();
        let mut whole: Vec<Row> = (0..4).map(|r| vec![mixed.values()[r].clone(), place.values()[r].clone()]).collect();
        whole.sort_by(|a, b| crate::exec::compare_rows(&up, a, b));
        let expected: Vec<String> = whole.iter().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")).collect();
        assert_eq!(made, expected);
        let missing = [SortKey { column: 2, descending: false }];
        assert_eq!(sorted_of(&missing, vec![mixed, place], 4_usize), Err(Code::Api));
        assert_eq!(sorted_of(&missing, vec![], 0_usize), Ok(vec![]), "no row, none refused");
    }

    /// Verifies: [DESIGN] RULES.md D31: cells compare as their values do:
    /// integers of one scale by their units, of two by value; values in
    /// place; texts padded; nulls as ORDER BY places them.
    #[test]
    fn cells_compare_as_their_values() {
        let pairs = [
            (Cell::Exact(15_i64, 1_u32), Cell::Exact(2_i64, 0_u32)),
            (Cell::Exact(3_i64, 0_u32), Cell::Exact(2_i64, 0_u32)),
            (Cell::Text("a"), Cell::Text("a  ")),
            (Cell::Approx(1.0_f64), Cell::Null),
            (Cell::Null, Cell::Exact(1_i64, 0_u32)),
        ];
        for (x, y) in pairs {
            assert_eq!(cell_cmp(x, y), sort_cmp(&x.to_value(), &y.to_value()), "{x:?} {y:?}");
        }
        let (a, b) = (Value::Char("b".to_owned()), Value::Char("a".to_owned()));
        assert_eq!(cell_cmp(Cell::Value(&a), Cell::Value(&b)), Ordering::Greater);
    }

    /// Verifies: [DESIGN] RULES.md D38: a batch is counted as holding its
    /// vectors' bytes: integers' and numbers' 8 bytes each and their nulls,
    /// values as rows count them, texts their bytes and ends, a store's
    /// dictionary its codes and entries.
    #[test]
    fn batches_count_the_bytes_their_vectors_hold() {
        let ints = Column::Ints(Ints::new(vec![1_i64, 2_i64], 0_u32, Some(vec![false, true])));
        assert_eq!(column_bytes(&ints), 18_usize);
        assert_eq!(column_bytes(&Column::Floats(Floats::new(vec![1.0_f64], None))), 8_usize);
        let values = Column::Values(vec![Value::Char("abc".to_owned())]);
        assert_eq!(column_bytes(&values), size_of::<Value>() + 3_usize);
        let texts = Texts::new("abcd".to_owned(), vec![2_usize, 4_usize], None);
        assert_eq!(column_bytes(&Column::Texts(texts.clone())), 4_usize + 2_usize * size_of::<usize>());
        let coded = Texts::coded(Coded { codes: vec![0_u32, 1_u32, 1_u32], entries: texts }, None);
        assert_eq!(column_bytes(&Column::Texts(coded)), 3_usize * 4_usize + 4_usize + 2_usize * size_of::<usize>());
        let batch = Batch { columns: vec![ints, values], rows: 2 };
        assert_eq!(batch_bytes(&batch), 18_usize + size_of::<Value>() + 3_usize);
    }
}
