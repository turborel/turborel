//! \[DESIGN\] RULES.md D38: a spilled sort's runs, and their merge. A run
//! is written as chunks of its rows, each a record, their vectors and each
//! row's head beside them (`chunk.rs`): a tag of the kind of its first
//! key's prefix (`prefix.rs`), if its value has one, and of whether the
//! prefix holds the value whole; and the prefix. The merge reads each run
//! a batch of records at a time (`SpillRun::into_batches`), each chunk's
//! vectors once, and orders the runs' next rows by their prefixes,
//! comparing the rows' values where they lie in their vectors only when
//! prefixes of kinds alike tie, or either has none; a row is made of its
//! values only when it is given. A pass that writes a run again writes the
//! rows merged as chunks of as many rows as its runs' first chunks held,
//! the most of them. Rows of equal prefixes held whole are compared by the
//! other keys alone. A tie goes to the earlier run, so the merge is
//! stable, and its order is the one comparing the rows whole gives
//! (`compare_rows`).

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

use super::chunk::{Chunk, Pick, chunk, put_chunk};
use super::prefix::{Kind, Prefix, alike};
use super::vectors::{compare_cells, row_of};
use crate::error::{Code, Result, SqlError};
use crate::logical::SortKey;
use crate::spill::{Batch, Batches, SpillRun};
use crate::storage::Row;
use crate::value::types::TypeRegistry;

/// A tag's bit saying its prefix holds its value whole; the rest are its
/// kind's code (`Kind::code`), 0 if it has none.
const WHOLE: u8 = 0x80;

/// What a merge orders rows by: the keys, and the types beyond the
/// standard its rows' values may be of, to decode them.
pub(super) struct Order {
    keys: Vec<SortKey>,
    types: TypeRegistry,
}

impl Order {
    /// An order by `keys`, of rows of values of `types`.
    pub(super) fn new(keys: &[SortKey], types: TypeRegistry) -> Order {
        Order { keys: keys.to_vec(), types }
    }
}

/// The head of a row whose first key's value has `prefix`: its tag and
/// its prefix.
pub(super) fn head(prefix: Option<Prefix>) -> (u8, u64) {
    prefix.map_or((0, 0), |p| (if p.whole { p.kind.code() | WHOLE } else { p.kind.code() }, p.bits))
}

/// A chunk shorter than its rows say.
fn short() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] a sorted run's chunk is short")
}

/// The rows of sorted `runs`, merged in `order`, each run's pages freed
/// as they are read.
///
/// # Errors
///
/// The runs' own; SQLCODE -916 of a record not a chunk.
pub(super) fn merge(order: &Arc<Order>, runs: Vec<Box<dyn SpillRun>>) -> Result<Merge> {
    let mut sources = Vec::with_capacity(runs.len());
    for run in runs {
        let records = Batch::default();
        sources.push(Source { batches: run.into_batches()?, records, record: 0, chunk: Arc::new(empty()), next: 0 });
    }
    let mut merge = Merge { order: Arc::clone(order), sources, heads: BinaryHeap::new(), each: 0 };
    for source in 0..merge.sources.len() {
        merge.refill(source)?;
        let rows = merge.sources.get(source).map_or(0, |s| s.chunk.batch.rows);
        merge.each = merge.each.max(rows);
    }
    Ok(merge)
}

/// A chunk of no row.
fn empty() -> Chunk {
    Chunk { tags: Vec::new(), prefixes: Vec::new(), batch: crate::storage::Batch { columns: Vec::new(), rows: 0 } }
}

/// A run being merged: its batches of records, the one being read and
/// its next record's place in it, and the chunk being read and its next
/// row's place in it.
struct Source {
    batches: Batches,
    records: Batch,
    record: usize,
    chunk: Arc<Chunk>,
    next: usize,
}

/// Runs being merged: each one's next row in the heap, first the least;
/// and the rows of a chunk a pass writes.
pub(super) struct Merge {
    order: Arc<Order>,
    sources: Vec<Source>,
    heads: BinaryHeap<Head>,
    each: usize,
}

impl Merge {
    /// Puts the next row of `source`, if any, in the heap.
    fn refill(&mut self, source: usize) -> Result<()> {
        let Some(from) = self.sources.get_mut(source) else { return Ok(()) };
        while from.next >= from.chunk.batch.rows {
            while from.record >= from.records.len() {
                let Some(records) = from.batches.next() else { return Ok(()) };
                (from.records, from.record) = (records?, 0);
            }
            let record = from.records.get(from.record).ok_or_else(short)?;
            (from.chunk, from.next) = (Arc::new(chunk(record, &self.order.types)?), 0);
            from.record = from.record.saturating_add(1);
        }
        let at = from.next;
        from.next = at.saturating_add(1);
        let tag = *from.chunk.tags.get(at).ok_or_else(short)?;
        let prefix = *from.chunk.prefixes.get(at).ok_or_else(short)?;
        let (chunk, order) = (Arc::clone(&from.chunk), Arc::clone(&self.order));
        self.heads.push(Head { tag, prefix, chunk, at, source, order });
        Ok(())
    }

    /// Writes the rows, merged, to `run`, as chunks of as many rows as its
    /// runs' first chunks held, the most of them: none made of its values.
    ///
    /// # Errors
    ///
    /// The runs' own.
    pub(super) fn write_to(mut self, run: &mut dyn SpillRun) -> Result<()> {
        let (mut pending, mut out) = (Vec::with_capacity(self.each), Vec::new());
        while let Some(head) = self.heads.pop() {
            self.refill(head.source)?;
            pending.push(head);
            if pending.len() >= self.each.max(1) {
                written(&mut pending, run, &mut out)?;
            }
        }
        written(&mut pending, run, &mut out)
    }
}

/// Writes `pending`, if any, to `run` as a chunk, made in `out`; then
/// none are pending.
fn written(pending: &mut Vec<Head>, run: &mut dyn SpillRun, out: &mut Vec<u8>) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let heads: Vec<(u8, u64)> = pending.iter().map(|h| (h.tag, h.prefix)).collect();
    let rows: Vec<Pick<'_>> = pending.iter().map(|h| (&h.chunk.batch, h.at)).collect();
    put_chunk(&heads, &rows, out);
    run.push(out)?;
    pending.clear();
    Ok(())
}

impl Iterator for Merge {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        let head = self.heads.pop()?;
        let row = row_of(&head.chunk.batch, head.at);
        Some(self.refill(head.source).map(|()| row))
    }
}

/// A run's next row, read where it lies in its chunk. The heap's greatest
/// is the least row, of the earliest run among equal ones.
struct Head {
    tag: u8,
    prefix: u64,
    chunk: Arc<Chunk>,
    at: usize,
    source: usize,
    order: Arc<Order>,
}

impl Head {
    /// Its prefix's kind, if it has one.
    fn kind(&self) -> Option<Kind> {
        Kind::of_code(self.tag & !WHOLE)
    }

    /// The order of its row and `other`'s [X3.135-1986 §8.3 GR3b]: by
    /// their prefixes, if of kinds alike and unequal; else by the keys
    /// after the first, if both prefixes hold their values whole, or by
    /// every key, compared where the values lie.
    fn order(&self, other: &Head) -> Ordering {
        let by_prefix = self.kind().zip(other.kind()).is_some_and(|(a, b)| alike(a, b));
        if by_prefix && self.prefix != other.prefix {
            return self.prefix.cmp(&other.prefix);
        }
        let keys = &self.order.keys;
        let whole = by_prefix && self.tag & other.tag & WHOLE != 0;
        let ties = if whole { keys.get(1..).unwrap_or_default() } else { keys.as_slice() };
        compare_cells(ties, (&self.chunk.batch, self.at), (&other.chunk.batch, other.at))
    }
}

impl Ord for Head {
    fn cmp(&self, other: &Head) -> Ordering {
        other.order(self).then(other.source.cmp(&self.source))
    }
}

impl PartialOrd for Head {
    fn partial_cmp(&self, other: &Head) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Head {
    fn eq(&self, other: &Head) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Head {}

#[cfg(test)]
mod tests {
    use super::super::compare_rows;
    use super::super::prefix::prefix_of;
    use super::*;
    use crate::spill::testing::{Held, areas};
    use crate::spill::{Area, SpillArea};
    use crate::storage::{Batch as Vectors, Column};
    use crate::value::{Binary, Decimal, Value};

    /// A spill area whose runs are held in memory.
    fn area() -> Box<dyn SpillArea> {
        areas(Some(Arc::new(Held { broken: false })), 1 << 20).area(Area::new(1 << 20, 4096)).unwrap()
    }

    /// `rows` as a batch of vectors of their values.
    fn vectors(rows: &[Row]) -> Vectors {
        let width = rows.first().map_or(0, Vec::len);
        let columns = (0..width).map(|c| Column::Values(rows.iter().map(|r| r[c].clone()).collect())).collect();
        Vectors { columns, rows: rows.len() }
    }

    /// Writes `rows`, sorted by `keys`, to `run` in chunks of 3 rows.
    fn write(keys: &[SortKey], rows: &[Row], run: &mut dyn SpillRun) {
        let mut out = Vec::new();
        for chunk in rows.chunks(3) {
            let batch = vectors(chunk);
            let picks: Vec<Pick<'_>> = (0..chunk.len()).map(|r| (&batch, r)).collect();
            let heads: Vec<(u8, u64)> =
                chunk.iter().map(|row| head(keys.first().and_then(|k| prefix_of(&row[k.column], k.descending)))).collect();
            put_chunk(&heads, &picks, &mut out);
            run.push(&out).unwrap();
        }
    }

    /// Runs of `rows`, `each` to a run, each sorted by `keys`, stably, and
    /// written as a spilled sort writes them.
    fn runs(keys: &[SortKey], rows: &[Row], each: usize) -> Vec<Box<dyn SpillRun>> {
        let area = area();
        rows.chunks(each)
            .map(|chunk| {
                let mut sorted = chunk.to_vec();
                sorted.sort_by(|a, b| compare_rows(keys, a, b));
                let mut run = area.run().unwrap();
                write(keys, &sorted, &mut *run);
                run
            })
            .collect()
    }

    /// `rows` sorted by `keys` in runs of `each`, merged in one pass, and in
    /// passes of two runs at a time; and as comparing them whole, stably.
    fn merged(keys: &[SortKey], rows: &[Row], each: usize) -> (Vec<Row>, Vec<Row>, Vec<Row>) {
        let order = Arc::new(Order::new(keys, TypeRegistry::default()));
        let once = merge(&order, runs(keys, rows, each)).unwrap().collect::<Result<Vec<Row>>>().unwrap();
        let mut passes = runs(keys, rows, each);
        while passes.len() > 2 {
            let mut next = Vec::new();
            let mut rest = passes.into_iter();
            loop {
                let group: Vec<Box<dyn SpillRun>> = rest.by_ref().take(2).collect();
                if group.is_empty() {
                    break;
                }
                let mut run = area().run().unwrap();
                merge(&order, group).unwrap().write_to(&mut *run).unwrap();
                next.push(run);
            }
            passes = next;
        }
        let twice = merge(&order, passes).unwrap().collect::<Result<Vec<Row>>>().unwrap();
        let mut whole = rows.to_vec();
        whole.sort_by(|a, b| compare_rows(keys, a, b));
        (once, twice, whole)
    }

    /// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §8.3 GR3b]: runs of
    /// chunks of vectors merged by their rows' prefixes, in one pass or in
    /// several, each writing chunks again, give the rows in the order
    /// comparing them whole gives, stably: ASC and DESC, nulls among them,
    /// of integers whose prefixes hold them whole, ties broken by the next
    /// key; of strings longer than a prefix, alike in it; of binary strings
    /// long and short; of exact numbers past 64 bits; of no key, the runs'
    /// order kept.
    #[test]
    fn merged_by_prefixes_as_compared_whole() {
        let int = |n: i128| Value::Exact(Decimal::new(n, 0_u32));
        let columns: [Box<dyn Fn(i128) -> Value>; 4] = [
            Box::new(move |n| if n % 7_i128 == 0 { Value::Null } else { int(n * 7919_i128 % 13_i128) }),
            Box::new(|n| if n % 5_i128 == 0 { Value::Null } else { Value::Char(format!("a shared start {}", n * 31_i128 % 11_i128)) }),
            Box::new(|n| Value::Binary(Binary::new(vec![0xA5; usize::try_from(n * 13_i128 % 40_i128).unwrap()]))),
            Box::new(move |n| int(if n % 2_i128 == 0 { i128::MAX - n % 3_i128 } else { i128::MIN + n % 3_i128 })),
        ];
        for column in &columns {
            let rows: Vec<Row> = (0_i128..60_i128).map(|n| vec![column(n), int(n % 3_i128), int(n)]).collect();
            for descending in [false, true] {
                let first = SortKey { column: 0, descending };
                for keys in [vec![first], vec![first, SortKey { column: 1, descending: !descending }]] {
                    let (once, twice, whole) = merged(&keys, &rows, 7);
                    assert_eq!(format!("{once:?}"), format!("{whole:?}"), "{keys:?}");
                    assert_eq!(format!("{twice:?}"), format!("{whole:?}"), "{keys:?}, in passes");
                }
            }
        }
        let rows: Vec<Row> = (0_i128..20_i128).map(|n| vec![int(n % 4_i128)]).collect();
        let (once, twice, whole) = merged(&[], &rows, 6);
        assert_eq!(format!("{once:?} {twice:?}"), format!("{whole:?} {whole:?}"), "no key");
    }

    /// Verifies: [DESIGN] RULES.md D38: a row whose tag is of a kind no
    /// prefix has is ordered by its values whole; a record not a chunk, or
    /// a chunk cut short, fails the merge as its run is read (SQLCODE
    /// -916).
    #[test]
    fn records_unknown_or_broken() {
        let keys = [SortKey { column: 0, descending: false }];
        let order = Arc::new(Order::new(&keys, TypeRegistry::default()));
        let (area, mut out) = (area(), Vec::new());
        let mut unknown = Vec::new();
        for n in [3_i128, 1_i128] {
            let mut run = area.run().unwrap();
            let batch = vectors(&[vec![Value::Exact(Decimal::new(n, 0_u32))]]);
            put_chunk(&[(WHOLE | 0x7f_u8, 0_u64)], &[(&batch, 0_usize)], &mut out);
            run.push(&out).unwrap();
            unknown.push(run);
        }
        let rows = merge(&order, unknown).unwrap().collect::<Result<Vec<Row>>>().unwrap();
        assert_eq!(
            format!("{rows:?}"),
            format!("{:?}", [[Value::Exact(Decimal::new(1_i128, 0_u32))], [Value::Exact(Decimal::new(3_i128, 0_u32))]])
        );
        let mut short = area.run().unwrap();
        short.push(&[1_u8, 2_u8]).unwrap();
        assert_eq!(merge(&order, vec![short]).err().map(|e| e.code), Some(Code::Corrupt));
        let batch = vectors(&[vec![Value::Char("a string longer than a prefix".into())]]);
        put_chunk(&[(0_u8, 0_u64)], &[(&batch, 0_usize)], &mut out);
        out.truncate(out.len() - 3);
        let mut cut = area.run().unwrap();
        cut.push(&out).unwrap();
        assert_eq!(merge(&order, vec![cut]).err().map(|e| e.code), Some(Code::Corrupt));
    }
}
