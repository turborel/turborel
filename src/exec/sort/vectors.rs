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
//! written as a run of chunks of vectors (`chunk.rs`), each about a spill
//! batch's bytes, and the runs merged as `sort.rs` merges them. A sort of
//! rows past its work area writes them so too, each work area's worth a
//! batch of vectors of their values (`written_rows`).

use std::cmp::Ordering;

use super::chunk::{Pick, put_chunk};
use super::merge::head;
use super::prefix::{Kind, joined, prefix_of_cell};
use super::{Work, merged};
use crate::db::State;
use crate::error::Result;
use crate::exec::batch::Batches;
use crate::exec::{RowIter, broken, failed};
use crate::logical::SortKey;
use crate::spill::SpillRun;
use crate::storage::{Batch, Cell, Column, Row};
use crate::txn::TxId;
use crate::value::{Value, sort_cmp};

/// \[DESIGN\] The fewest and the most rows of a chunk of a run.
const CHUNK_ROWS: (usize, usize) = (16, 1 << 16);

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
        self.bytes = self.bytes.saturating_add(batch.bytes()).saturating_add(places);
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
    /// in chunks of vectors (`chunk.rs`) of about a spill batch's bytes.
    fn written(mut self, keys: &[SortKey], work: &mut Work) -> Result<Box<dyn SpillRun>> {
        self.sort(keys);
        let each = chunk_rows(self.bytes, self.places.len(), work.batch());
        let (mut run, mut out) = (work.run()?, Vec::new());
        for places in self.places.chunks(each) {
            let rows: Vec<Pick<'_>> = places.iter().filter_map(|p| self.batches.get(p.batch).map(|b| (b, p.row))).collect();
            let heads: Vec<(u8, u64)> =
                rows.iter().map(|&(b, r)| head(keys.first().and_then(|k| prefix_of_cell(b.cell(k.column, r), k.descending)))).collect();
            put_chunk(&heads, &rows, &mut out);
            run.push(&out)?;
        }
        Ok(run)
    }
}

/// The rows a chunk of a run holds, of rows that hold `bytes` all told, so
/// it holds about `batch` bytes: within `CHUNK_ROWS`.
fn chunk_rows(bytes: usize, rows: usize, batch: usize) -> usize {
    let each = bytes.checked_div(rows).unwrap_or(1).max(1);
    batch.checked_div(each).unwrap_or(0).clamp(CHUNK_ROWS.0, CHUNK_ROWS.1)
}

/// `rows`, sorted by `keys`, written as a run of `work`'s spill area as
/// a sort of batches writes them, of a batch of vectors of their values.
pub(super) fn written_rows(keys: &[SortKey], rows: Vec<Row>, work: &mut Work) -> Result<Box<dyn SpillRun>> {
    let width = rows.first().map_or(0, Vec::len);
    let count = rows.len();
    let mut columns: Vec<Vec<Value>> = (0..width).map(|_| Vec::with_capacity(count)).collect();
    for row in rows {
        for (column, v) in columns.iter_mut().zip(row) {
            column.push(v);
        }
    }
    let batch = Batch { columns: columns.into_iter().map(Column::Values).collect(), rows: count };
    let mut held = Held::default();
    held.add(keys, batch, &(0..count).collect::<Vec<usize>>());
    held.written(keys, work)
}

/// Row `r` of `batch`, made of its values.
pub(super) fn row_of(batch: &Batch, r: usize) -> Row {
    (0..batch.columns.len()).map(|at| batch.cell(at, r).to_value()).collect()
}

/// [X3.135-1986 §8.3 GR3b] The order of row `ra` of `a` and row `rb` of
/// `b` under `keys`, as `compare_rows` orders the rows of their values.
pub(super) fn compare_cells(keys: &[SortKey], (a, ra): (&Batch, usize), (b, rb): (&Batch, usize)) -> Ordering {
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

// Unit tests: sorts of batches whose rows the integration tests do not
// make (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MemoryCatalog;
    use crate::error::Code;
    use crate::storage::Ints;
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
}
