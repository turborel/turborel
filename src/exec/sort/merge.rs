//! \[DESIGN\] RULES.md D38: a spilled sort's runs, and their merge. Each
//! row is written as a record beside the prefix of its first key
//! (`prefix.rs`): a tag of the prefix's kind, if its value has one, and of
//! whether it holds the value whole; the prefix; then the row. The merge
//! reads each run a batch at a time (`SpillRun::into_batches`), each record
//! where it lies, and orders the runs' next records by their prefixes,
//! decoding a record's row only when its prefix ties another of a kind
//! alike, or either has none, and when the row is given; a pass that
//! writes a run again copies each record as it is. Rows of equal prefixes
//! held whole are compared by the other keys alone. A tie goes to the
//! earlier run, so the merge is stable, and its order is the one comparing
//! the rows whole gives (`compare_rows`). Split from sort.rs (RULES.md
//! R-SIZE-2).

use std::cell::OnceCell;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

use super::compare_rows;
use super::prefix::{Kind, alike, prefix_of};
use crate::error::{Code, Result, SqlError};
use crate::logical::SortKey;
use crate::spill::{Batch, Batches, SpillRun, record_row};
use crate::storage::Row;
use crate::value::codec::put_row;
use crate::value::types::TypeRegistry;

/// The bytes before a record's row: its tag, and its prefix, a `u64`.
const HEAD_BYTES: usize = 9;

/// The bytes of a record's prefix, a `u64`, after its tag.
const PREFIX_LEN: usize = 8;

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

/// `row`'s record, sorted by `keys`, written to `out`, emptied first.
pub(super) fn record(keys: &[SortKey], row: &Row, out: &mut Vec<u8>) {
    out.clear();
    let prefix = keys.first().and_then(|k| row.get(k.column).and_then(|v| prefix_of(v, k.descending)));
    let (tag, bits) = prefix.map_or((0, 0), |p| (if p.whole { p.kind.code() | WHOLE } else { p.kind.code() }, p.bits));
    out.push(tag);
    out.extend_from_slice(&bits.to_be_bytes());
    put_row(out, row);
}

/// A record too short to hold its prefix.
fn short() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] a sorted run's record is short")
}

/// The records of sorted `runs`, merged in `order`, each run's pages freed
/// as they are read.
///
/// # Errors
///
/// The runs' own; SQLCODE -916 of a record too short.
pub(super) fn merge(order: &Arc<Order>, runs: Vec<Box<dyn SpillRun>>) -> Result<Merge> {
    let mut sources = Vec::with_capacity(runs.len());
    for run in runs {
        sources.push(Source { batches: run.into_batches()?, batch: Arc::new(Batch::default()), next: 0 });
    }
    let mut merge = Merge { order: Arc::clone(order), sources, heads: BinaryHeap::new() };
    for source in 0..merge.sources.len() {
        merge.refill(source)?;
    }
    Ok(merge)
}

/// A run being merged: its batches, the one being read, and its next
/// record's place in it.
struct Source {
    batches: Batches,
    batch: Arc<Batch>,
    next: usize,
}

/// Runs being merged: each one's next record in the heap, first the least.
pub(super) struct Merge {
    order: Arc<Order>,
    sources: Vec<Source>,
    heads: BinaryHeap<Head>,
}

impl Merge {
    /// Puts the next record of `source`, if any, in the heap.
    fn refill(&mut self, source: usize) -> Result<()> {
        let Some(from) = self.sources.get_mut(source) else { return Ok(()) };
        while from.next >= from.batch.len() {
            let Some(batch) = from.batches.next() else { return Ok(()) };
            (from.batch, from.next) = (Arc::new(batch?), 0);
        }
        let at = from.next;
        from.next = at.saturating_add(1);
        let record = from.batch.get(at).ok_or_else(short)?;
        let (&tag, rest) = record.split_first().ok_or_else(short)?;
        let prefix = rest.first_chunk::<PREFIX_LEN>().map(|b| u64::from_be_bytes(*b)).ok_or_else(short)?;
        let (batch, order) = (Arc::clone(&from.batch), Arc::clone(&self.order));
        self.heads.push(Head { tag, prefix, batch, at, source, order, row: OnceCell::new() });
        Ok(())
    }

    /// Writes the records, merged, to `run`, as they are: none decoded but
    /// to order those whose prefixes tie.
    ///
    /// # Errors
    ///
    /// The runs' own.
    pub(super) fn write_to(mut self, run: &mut dyn SpillRun) -> Result<()> {
        while let Some(head) = self.heads.pop() {
            run.push(head.record())?;
            self.refill(head.source)?;
        }
        Ok(())
    }
}

impl Iterator for Merge {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        let mut head = self.heads.pop()?;
        let row = match head.row.take().flatten() {
            Some(row) => Ok(row),
            None => head.decoded(),
        };
        Some(row.and_then(|row| self.refill(head.source).map(|()| row)))
    }
}

/// A run's next record, read where it lies in its batch, its row decoded
/// once, if it must be. The heap's greatest is the least row, of the
/// earliest run among equal ones.
struct Head {
    tag: u8,
    prefix: u64,
    batch: Arc<Batch>,
    at: usize,
    source: usize,
    order: Arc<Order>,
    /// Its row, once decoded; none if it could not be, the error raised as
    /// it is given.
    row: OnceCell<Option<Row>>,
}

impl Head {
    /// Its record, as written.
    fn record(&self) -> &[u8] {
        self.batch.get(self.at).unwrap_or_default()
    }

    /// Its record's row, decoded.
    fn decoded(&self) -> Result<Row> {
        record_row(self.record().get(HEAD_BYTES..).unwrap_or_default(), &self.order.types)
    }

    /// Its row, decoded the first time it is asked for.
    fn row(&self) -> Option<&Row> {
        self.row.get_or_init(|| self.decoded().ok()).as_ref()
    }

    /// Its prefix's kind, if it has one.
    fn kind(&self) -> Option<Kind> {
        Kind::of_code(self.tag & !WHOLE)
    }

    /// The order of its row and `other`'s [X3.135-1986 §8.3 GR3b]: by
    /// their prefixes, if of kinds alike and unequal; else by the keys
    /// after the first, if both prefixes hold their values whole, or by
    /// every key, their rows decoded.
    fn order(&self, other: &Head) -> Ordering {
        let by_prefix = self.kind().zip(other.kind()).is_some_and(|(a, b)| alike(a, b));
        if by_prefix && self.prefix != other.prefix {
            return self.prefix.cmp(&other.prefix);
        }
        let keys = &self.order.keys;
        let whole = by_prefix && self.tag & other.tag & WHOLE != 0;
        let ties = if whole { keys.get(1..).unwrap_or_default() } else { keys.as_slice() };
        if ties.is_empty() {
            return Ordering::Equal;
        }
        self.row().zip(other.row()).map_or(Ordering::Equal, |(a, b)| compare_rows(ties, a, b))
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
    use super::*;
    use crate::spill::testing::{Held, areas};
    use crate::spill::{Area, SpillArea};
    use crate::value::{Bits, Decimal, Value};

    /// A spill area whose runs are held in memory.
    fn area() -> Box<dyn SpillArea> {
        areas(Some(Arc::new(Held { broken: false })), 1 << 20).area(Area::new(1 << 20, 4096)).unwrap()
    }

    /// Runs of `rows`, `each` to a run, each sorted by `keys`, stably, and
    /// written as a spilled sort writes them.
    fn runs(keys: &[SortKey], rows: &[Row], each: usize) -> Vec<Box<dyn SpillRun>> {
        let (area, mut out) = (area(), Vec::new());
        rows.chunks(each)
            .map(|chunk| {
                let mut sorted = chunk.to_vec();
                sorted.sort_by(|a, b| compare_rows(keys, a, b));
                let mut run = area.run().unwrap();
                for row in &sorted {
                    record(keys, row, &mut out);
                    run.push(&out).unwrap();
                }
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

    /// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §8.3 GR3b]: runs merged
    /// by their records' prefixes, in one pass or in several, each record
    /// copied as it is, give the rows in the order comparing them whole
    /// gives, stably: ASC and DESC, nulls among them, of integers whose
    /// prefixes hold them whole, ties broken by the next key; of strings
    /// longer than a prefix, alike in it; of bit strings long and short; of
    /// exact numbers past 64 bits; of no key, the runs' order kept.
    #[test]
    fn merged_by_prefixes_as_compared_whole() {
        let int = |n: i128| Value::Exact(Decimal::new(n, 0_u32));
        let columns: [Box<dyn Fn(i128) -> Value>; 4] = [
            Box::new(move |n| if n % 7_i128 == 0 { Value::Null } else { int(n * 7919_i128 % 13_i128) }),
            Box::new(|n| if n % 5_i128 == 0 { Value::Null } else { Value::Char(format!("a shared start {}", n * 31_i128 % 11_i128)) }),
            Box::new(|n| Value::Bits(Bits::of_bits(&"10".repeat(usize::try_from(n * 13_i128 % 40_i128).unwrap())).unwrap())),
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

    /// Verifies: [DESIGN] RULES.md D38: a record of a kind no prefix has is
    /// ordered by its row whole; a record too short for its prefix fails
    /// the merge, and one whose row cannot be read fails it as it is given,
    /// though compared before (SQLCODE -916).
    #[test]
    fn records_unknown_or_broken() {
        let keys = [SortKey { column: 0, descending: false }];
        let order = Arc::new(Order::new(&keys, TypeRegistry::default()));
        let (area, mut out) = (area(), Vec::new());
        let mut unknown = Vec::new();
        for n in [3_i128, 1_i128] {
            let mut run = area.run().unwrap();
            record(&keys, &vec![Value::Exact(Decimal::new(n, 0_u32))], &mut out);
            *out.first_mut().unwrap() = WHOLE | 0x7f_u8;
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
        let broken: Vec<Box<dyn SpillRun>> = (0_u8..2_u8)
            .map(|_| {
                let mut run = area.run().unwrap();
                record(&keys, &vec![Value::Char("a string longer than a prefix".into())], &mut out);
                out.truncate(HEAD_BYTES.saturating_add(3));
                run.push(&out).unwrap();
                run
            })
            .collect();
        let mut merged = merge(&order, broken).unwrap();
        assert_eq!(merged.next().map(|r| r.err().map(|e| e.code)), Some(Some(Code::Corrupt)));
    }
}
