//! \[DESIGN\] RULES.md D38, D364: a spilled sort's runs, and their merge. A
//! run is written as chunks of its rows, each a record, their vectors and
//! their normal keys beside them (`chunk.rs`). The merge reads each run a
//! batch of records at a time (`SpillRun::into_batches`), each chunk's
//! vectors once, and gives the runs' next rows in order by a tree of losers
//! (`merge/losers.rs`). Two rows of keys of one layout are ordered by their
//! keys' bytes, and only if those are equal and may hide a difference, by
//! the rows' values where they lie in their vectors; rows of keys of two
//! layouts, or of none, by their values. A row is made of its values only
//! when it is given. A pass that writes a run again writes the rows merged
//! as chunks of as many rows as its runs' first chunks held, the most of
//! them, with their keys. A tie goes to the earlier run, so the merge is
//! stable, and its order is the one comparing the rows whole gives
//! (`compare_rows`).

mod losers;

use std::cmp::Ordering;
use std::sync::Arc;

use super::chunk::{Chunk, Keys, Pick, chunk, put_chunk};
use super::vectors::{compare_cells, row_of};
use crate::error::{Code, Result, SqlError};
use crate::logical::SortKey;
use crate::spill::{Batch, Batches, SpillRun, SpilledLobs};
use crate::storage::Row;
use crate::value::types::TypeRegistry;
use losers::Losers;

/// What a merge orders rows by: the keys, the types beyond the
/// standard its rows' values may be of, to decode them, and the sources
/// of what they name.
pub(super) struct Order {
    keys: Vec<SortKey>,
    types: TypeRegistry,
    lobs: SpilledLobs,
}

impl Order {
    /// An order by `keys`, of rows of values of `types`, naming what
    /// `lobs` holds.
    pub(super) fn new(keys: &[SortKey], types: TypeRegistry, lobs: SpilledLobs) -> Order {
        Order { keys: keys.to_vec(), types, lobs }
    }
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
    let mut each = 0_usize;
    for run in runs {
        let mut source = Source { batches: run.into_batches()?, records: Batch::default(), record: 0, chunk: Arc::new(empty()), at: 0 };
        source.next_chunk(order)?;
        each = each.max(source.chunk.batch.rows);
        sources.push(source);
    }
    let losers = Losers::new(sources.len(), |a, b| before(&order.keys, &sources, a, b));
    Ok(Merge { order: Arc::clone(order), sources, losers, each })
}

/// A chunk of no row.
fn empty() -> Chunk {
    Chunk { layout: 0, truncates: false, width: 0, keys: Vec::new(), batch: crate::storage::Batch { columns: Vec::new(), rows: 0 } }
}

/// A run being merged: its batches of records, the one being read and
/// its next record's place in it, and the chunk being read and its next
/// row's place in it.
struct Source {
    batches: Batches,
    records: Batch,
    record: usize,
    chunk: Arc<Chunk>,
    at: usize,
}

impl Source {
    /// Its next row, where it lies; none once it has given every one.
    fn head(&self) -> Option<(&Chunk, usize)> {
        (self.at < self.chunk.batch.rows).then_some((&*self.chunk, self.at))
    }

    /// Reads its next chunk of a row or more, if any, as `order` reads it.
    fn next_chunk(&mut self, order: &Order) -> Result<()> {
        self.at = 0;
        loop {
            while self.record >= self.records.len() {
                let Some(records) = self.batches.next() else {
                    self.chunk = Arc::new(empty());
                    return Ok(());
                };
                (self.records, self.record) = (records?, 0);
            }
            let record = self.records.get(self.record).ok_or_else(short)?;
            self.chunk = Arc::new(chunk(record, &order.types, &order.lobs)?);
            self.record = self.record.saturating_add(1);
            if self.chunk.batch.rows > 0 {
                return Ok(());
            }
        }
    }

    /// Moves past its next row.
    fn advance(&mut self, order: &Order) -> Result<()> {
        self.at = self.at.saturating_add(1);
        if self.at >= self.chunk.batch.rows { self.next_chunk(order) } else { Ok(()) }
    }
}

/// Whether the next row of source `a` of `sources` goes before source
/// `b`'s under `keys`. A run that has given every row goes last, and a tie
/// goes to the earlier run.
fn before(keys: &[SortKey], sources: &[Source], a: usize, b: usize) -> bool {
    let heads = sources.get(a).and_then(Source::head).zip(Some(sources.get(b).and_then(Source::head)));
    match heads {
        Some((x, Some(y))) => compared(keys, x, y).then(a.cmp(&b)).is_lt(),
        Some((_, None)) => true,
        None => sources.get(b).and_then(Source::head).is_none() && a < b,
    }
}

/// The order of row `ax` of chunk `x` and row `ay` of `y` under `keys`
/// [X3.135-1986 §8.3 GR3b]: by their keys, if of one layout, and by their
/// values if those are equal and may hide a difference, or of no layout.
fn compared(keys: &[SortKey], (x, ax): (&Chunk, usize), (y, ay): (&Chunk, usize)) -> Ordering {
    if x.layout != 0 && x.layout == y.layout {
        let by_keys = x.key(ax).cmp(y.key(ay));
        if by_keys.is_ne() || !x.truncates {
            return by_keys;
        }
    }
    compare_cells(keys, (&x.batch, ax), (&y.batch, ay))
}

/// Runs being merged: each one's next row a leaf of the tree of losers;
/// and the rows of a chunk a pass writes.
pub(super) struct Merge {
    order: Arc<Order>,
    sources: Vec<Source>,
    losers: Losers,
    each: usize,
}

impl Merge {
    /// The next row's chunk and place, and the source past it; none once
    /// every run has given every row.
    fn taken(&mut self) -> Result<Option<(Arc<Chunk>, usize)>> {
        let winner = self.losers.winner();
        let Some(source) = self.sources.get_mut(winner) else { return Ok(None) };
        let Some((_, at)) = source.head() else { return Ok(None) };
        let chunk = Arc::clone(&source.chunk);
        source.advance(&self.order)?;
        let (keys, sources) = (&self.order.keys, &self.sources);
        self.losers.replay(|a, b| before(keys, sources, a, b));
        Ok(Some((chunk, at)))
    }

    /// Writes the rows, merged, to `run`, as chunks of as many rows as its
    /// runs' first chunks held, the most of them: none made of its values.
    ///
    /// # Errors
    ///
    /// The runs' own.
    pub(super) fn write_to(mut self, run: &mut dyn SpillRun) -> Result<()> {
        let (mut pending, mut out) = (Vec::with_capacity(self.each), Vec::new());
        while let Some(taken) = self.taken()? {
            pending.push(taken);
            if pending.len() >= self.each.max(1) {
                written(&mut pending, run, (&mut out, &self.order.lobs))?;
            }
        }
        written(&mut pending, run, (&mut out, &self.order.lobs))
    }
}

/// Writes `pending`, if any, to `run` as a chunk, made in `out`, naming
/// what `lobs` holds, with their keys if all are of one layout; then none
/// are pending.
fn written(pending: &mut Vec<(Arc<Chunk>, usize)>, run: &mut dyn SpillRun, (out, lobs): (&mut Vec<u8>, &SpilledLobs)) -> Result<()> {
    let Some((first, _)) = pending.first() else { return Ok(()) };
    let (layout, truncates, width) = (first.layout, first.truncates, first.width);
    let alike = layout != 0 && pending.iter().all(|(c, _)| c.layout == layout);
    let bytes: Vec<u8> = if alike { pending.iter().flat_map(|(c, at)| c.key(*at).iter().copied()).collect() } else { Vec::new() };
    let keys = if alike { Keys { layout, truncates, width, bytes: &bytes } } else { Keys::default() };
    let rows: Vec<Pick<'_>> = pending.iter().map(|(c, at)| (&c.batch, *at)).collect();
    put_chunk(keys, &rows, out, lobs);
    run.push(out)?;
    pending.clear();
    Ok(())
}

impl Iterator for Merge {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        match self.taken() {
            Ok(Some((chunk, at))) => Some(Ok(row_of(&chunk.batch, at))),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::compare_rows;
    use super::super::normal::{Layout, Shape, encode_row, seen_rows};
    use super::*;
    use crate::spill::testing::{Held, areas};
    use crate::spill::{Area, SpillArea};
    use crate::storage::{Batch as Vectors, Column};
    use crate::value::{Binary, Decimal, SortOrder, Value};

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

    /// Writes `rows`, sorted by `keys`, to `run` in chunks of 3 rows, with
    /// keys of `layout` numbered `number` if given.
    fn write(rows: &[Row], layout: Option<(&Layout, u32)>, run: &mut dyn SpillRun) {
        let mut out = Vec::new();
        for chunk in rows.chunks(3) {
            let batch = vectors(chunk);
            let picks: Vec<Pick<'_>> = (0..chunk.len()).map(|r| (&batch, r)).collect();
            let mut bytes = Vec::new();
            let keys = layout.map_or_else(Keys::default, |(layout, number)| {
                for row in chunk {
                    let mut key = vec![0_u8; layout.width];
                    encode_row(layout, row, &mut key).unwrap();
                    bytes.extend(key);
                }
                Keys { layout: number, truncates: layout.truncates, width: layout.width, bytes: &bytes }
            });
            put_chunk(keys, &picks, &mut out, &SpilledLobs::new());
            run.push(&out).unwrap();
        }
    }

    /// Runs of `rows`, `each` to a run, each sorted by `keys`, stably, and
    /// written as a spilled sort writes them: with keys of one open layout
    /// if the rows have one, every third run of none, and the second of
    /// one of its own.
    fn runs(keys: &[SortKey], rows: &[Row], each: usize) -> Vec<Box<dyn SpillRun>> {
        let area = area();
        let layout = Layout::new(keys, &seen_rows(keys, rows), Shape::Open);
        rows.chunks(each)
            .enumerate()
            .map(|(n, chunk)| {
                let mut sorted = chunk.to_vec();
                sorted.sort_by(|a, b| compare_rows(keys, a, b));
                let mut run = area.run().unwrap();
                let number = if n == 1 { 2 } else { 1 };
                write(&sorted, layout.as_ref().filter(|_| n % 3 != 2).map(|l| (l, number)), &mut *run);
                run
            })
            .collect()
    }

    /// `rows` sorted by `keys` in runs of `each`, merged in one pass, and in
    /// passes of two runs at a time; and as comparing them whole, stably.
    fn merged(keys: &[SortKey], rows: &[Row], each: usize) -> (Vec<Row>, Vec<Row>, Vec<Row>) {
        let order = Arc::new(Order::new(keys, TypeRegistry::default(), SpilledLobs::new()));
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

    /// Verifies: [DESIGN] RULES.md D38, D364 [X3.135-1986 §8.3 GR3b]: runs
    /// of chunks of vectors merged by a tree of losers, by their rows' keys
    /// of one layout, of two, or of none, in one pass or in several, each
    /// writing chunks again, give the rows in the order comparing them
    /// whole gives, stably: ASC and DESC, nulls among them
    /// first or last [ISO/IEC 9075-2:2023 §10.10 GR1e ii], of integers,
    /// ties broken by the next key; of strings longer than their keys hold,
    /// alike in them; of binary strings long, short and empty; of exact
    /// numbers past 64 bits; of no key, the runs' order kept.
    #[test]
    fn merged_by_keys_as_compared_whole() {
        let int = |n: i128| Value::Exact(Decimal::new(n, 0_u32));
        let columns: [Box<dyn Fn(i128) -> Value>; 4] = [
            Box::new(move |n| if n % 7_i128 == 0 { Value::Null } else { int(n * 7919_i128 % 13_i128) }),
            Box::new(
                |n| if n % 5_i128 == 0 { Value::Null } else { Value::Char(format!("a shared start {}", n * 31_i128 % 11_i128).into()) },
            ),
            Box::new(|n| {
                if n % 6_i128 == 0 {
                    Value::Null
                } else {
                    Value::Binary(Binary::new(vec![0xA5; usize::try_from(n * 13_i128 % 40_i128).unwrap()]))
                }
            }),
            Box::new(move |n| int(if n % 2_i128 == 0 { i128::MAX - n % 3_i128 } else { i128::MIN + n % 3_i128 })),
        ];
        for column in &columns {
            let rows: Vec<Row> = (0_i128..60_i128).map(|n| vec![column(n), int(n % 3_i128), int(n)]).collect();
            for (descending, nulls_first) in [(false, false), (false, true), (true, false), (true, true)] {
                let first = SortKey { column: 0, descending, nulls_first };
                for keys in [vec![first], vec![first, SortKey::new(1, SortOrder::of(!descending, None))]] {
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

    /// Verifies: [DESIGN] RULES.md D38: a record not a chunk, or a chunk
    /// cut short, fails the merge as its run is read (SQLCODE -916).
    #[test]
    fn records_broken() {
        let keys = [SortKey::ascending(0)];
        let order = Arc::new(Order::new(&keys, TypeRegistry::default(), SpilledLobs::new()));
        let (area, mut out) = (area(), Vec::new());
        let mut short = area.run().unwrap();
        short.push(&[1_u8, 2_u8]).unwrap();
        assert_eq!(merge(&order, vec![short]).err().map(|e| e.code), Some(Code::Corrupt));
        let batch = vectors(&[vec![Value::Char("a string longer than a prefix".into())]]);
        put_chunk(Keys::default(), &[(&batch, 0_usize)], &mut out, &SpilledLobs::new());
        out.truncate(out.len() - 3);
        let mut cut = area.run().unwrap();
        cut.push(&out).unwrap();
        assert_eq!(merge(&order, vec![cut]).err().map(|e| e.code), Some(Code::Corrupt));
    }
}
