//! [ISO/IEC 9075:1992 §7.2, §7.10] The rows of a `<table value
//! constructor>`, and of EXCEPT and INTERSECT; UNION is UNION ALL, an
//! append, under duplicate elimination (`physical.rs`). \[DESIGN\] RULES.md
//! D38: EXCEPT and INTERSECT count the second operand's rows in memory
//! while they fit the work area, else partition both operands alike. But
//! for ALL, they keep the rows given already in a bag beside the counts,
//! their work area split in two, half the counts', half the bag's, so the
//! two hold no more than one work area. RULES.md D31: an operand that is a
//! stream holding every column of its rows is read by batch, each row's
//! key made of its vectors (`batch::keyed`), and a row made only of one
//! kept or partitioned. Split from exec.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;
use std::sync::Arc;

use super::batch::{Keyed, KeyedBatches, keyed, whole_width};
use super::partition::{Partitioner, merge_all, numbered, unnumbered};
use super::work::{Bag, ENTRY_BYTES, Work};
use super::{RowIter, Runner, batched, broken, eval, execute, failed};
use crate::db::State;
use crate::error::Result;
use crate::logical::SetOp;
use crate::physical::Plan;
use crate::spill::{Records, SpillRun, norm_row};
use crate::storage::Row;
use crate::txn::TxId;

/// [ISO/IEC 9075:1992 §7.2 GR1, GR2] The table value constructor `plan`:
/// a row for each of its lists, each expression evaluated over no row.
/// Never inlined into `execute`, whose frame every level of a plan adds to
/// the stack (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn values(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Values { rows, .. } = plan else { return failed(broken("a table value constructor is not one")) };
    let (empty, runner) = (Vec::new(), Runner::new(db, tx));
    match rows.iter().map(|r| r.iter().map(|e| eval(e, &empty, &runner)).collect::<Result<Row>>()).collect::<Result<Vec<Row>>>() {
        Ok(found) => Box::new(found.into_iter().map(Ok)),
        Err(e) => failed(e),
    }
}

/// A row as its duplicates know it [ISO/IEC 9075:1992 §7.10 GR1b i]:
/// values equal by comparison, nulls identical.
fn key(row: &Row) -> Vec<u8> {
    norm_row(row)
}

/// [ISO/IEC 9075:1992 §7.10 GR1b] EXCEPT or INTERSECT, `plan`: of a row
/// with m duplicates in the first operand and n in the second, EXCEPT
/// gives one if m > 0 and n = 0, and EXCEPT ALL max(m - n, 0); INTERSECT
/// one if m > 0 and n > 0, and INTERSECT ALL min(m, n). They are the first
/// operand's, in its order. The second operand is read first, and the
/// first only if it succeeded. Never inlined into `execute`, whose frame
/// every level of a plan adds to the stack (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn operation(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::SetOp { op, all, left, right } = plan else { return failed(broken("a set operation is not one")) };
    let how = Kept { intersect: op == SetOp::Intersect, all };
    match Operand::of(*right, db, tx).and_then(|second| counted_rows(second, (db, tx), how)) {
        Ok((Counted::Held(counts), _, given)) => match Operand::of(*left, db, tx) {
            Ok(first) => keeping(first, counts, how, given),
            Err(e) => failed(e),
        },
        Ok((Counted::Partitioned(parts), work, given)) => match Operand::of(*left, db, tx) {
            Ok(first) => partitioned(first, (parts, how), (work, given)),
            Err(e) => failed(e),
        },
        Err(e) => failed(e),
    }
}

/// An operand's rows: one at a time, or, of a stream holding every column
/// of its rows, by batch, each row keyed (`batch::keyed`).
enum Operand {
    Rows(RowIter),
    Keyed(KeyedBatches),
}

/// Rows or keys, each a row's or of a batch's, as they are read.
type Each<T> = Box<dyn Iterator<Item = Result<T>> + Send>;

impl Operand {
    /// The rows of `plan`, by batch where EXECUTOR.BATCHES allows and it
    /// is such a stream, as `tx` sees them.
    fn of(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Operand> {
        if batched(db) && whole_width(&plan, db).is_some() {
            return Ok(Operand::Keyed(keyed(plan, db, tx)?));
        }
        Ok(Operand::Rows(execute(plan, db, tx)))
    }

    /// Each row's key, counted once.
    fn keys(self) -> Each<(Vec<u8>, u64)> {
        match self {
            Operand::Rows(rows) => Box::new(rows.map(|r| r.map(|row| (key(&row), 1)))),
            Operand::Keyed(batches) => Box::new(batches.flat_map(|b| each(b, |keyed, n| (keyed.key(n).to_vec(), 1)))),
        }
    }

    /// Each row, with its key.
    fn rows(self) -> Each<(Vec<u8>, Row)> {
        match self {
            Operand::Rows(rows) => Box::new(rows.map(|r| r.map(|row| (key(&row), row)))),
            Operand::Keyed(batches) => Box::new(batches.flat_map(|b| each(b, |keyed, n| (keyed.key(n).to_vec(), keyed.row(n))))),
        }
    }
}

/// What `made` makes of each row of the batch `b`, in order, or its error.
fn each<T>(b: Result<Keyed>, made: impl Fn(&Keyed, usize) -> T) -> Vec<Result<T>> {
    match b {
        Ok(keyed) => (0..keyed.len()).map(|n| Ok(made(&keyed, n))).collect(),
        Err(e) => vec![Err(e)],
    }
}

/// Keys counted: in a bag, or, once they outgrew the work area,
/// partitioned by key, each record a count and a key.
enum Counted {
    Held(Bag),
    Partitioned(Partitioner),
}

/// The rows of `rows`, counted, in a work area of `db`, for the statement
/// of `tx`; and the work area of the bags of rows given already, of `how`.
/// Kept apart from `operation`, whose frame is on the stack while its
/// operands are made (RULES.md R-CF-6).
#[inline(never)]
fn counted_rows(rows: Operand, (db, tx): (&State, TxId), how: Kept) -> Result<(Counted, Work, Work)> {
    let mut work = Work::of(db, tx);
    // ALL keeps no bag of rows given, so takes the whole work area.
    let given = if how.all { work.another() } else { work.split() };
    let counts = counted(rows.keys(), &mut work, true)?;
    Ok((counts, work, given))
}

/// The keys `keys`, each with a count, counted in memory while they fit
/// `work`; past that, partitioned if `partition`, else in a bag, which
/// spills.
fn counted(mut keys: impl Iterator<Item = Result<(Vec<u8>, u64)>>, work: &mut Work, partition: bool) -> Result<Counted> {
    let (mut counts, mut bytes) = (HashMap::<Vec<u8>, u64>::new(), 0_usize);
    while let Some(r) = keys.next() {
        let (k, more) = r?;
        if !counts.contains_key(&k) {
            bytes = bytes.saturating_add(k.len()).saturating_add(ENTRY_BYTES);
        }
        let n = counts.entry(k).or_default();
        *n = n.saturating_add(more);
        if bytes > work.limit() {
            if !partition {
                return Ok(Counted::Held(bagged(counts.into_iter().map(Ok).chain(keys), work.another())?));
            }
            let mut parts = Partitioner::of(work)?;
            for rest in counts.into_iter().map(Ok).chain(keys) {
                let (held, count) = rest?;
                parts.push(&held, &counted_record(count, &held))?;
            }
            return Ok(Counted::Partitioned(parts));
        }
    }
    Ok(Counted::Held(bagged(counts.into_iter().map(Ok), work.another())?))
}

/// The keys `keys`, each with a count, in a bag of `work`.
fn bagged(keys: impl Iterator<Item = Result<(Vec<u8>, u64)>>, work: Work) -> Result<Bag> {
    let mut bag = Bag::new(work);
    for r in keys {
        let (k, n) = r?;
        bag.add_count(&k, n)?;
    }
    Ok(bag)
}

/// A count of a key, and the key, as a record.
fn counted_record(n: u64, key: &[u8]) -> Vec<u8> {
    [&n.to_le_bytes()[..], key].concat()
}

/// The key and the count of a record `counted_record` made.
fn uncounted(record: &[u8]) -> Result<(Vec<u8>, u64)> {
    let (n, k) = record.split_at_checked(8).ok_or_else(|| broken("a counted record is short"))?;
    Ok((k.to_vec(), <[u8; 8]>::try_from(n).map(u64::from_le_bytes).map_err(|_| broken("a counted record is short"))?))
}

/// The rows of `first` the set operation `how` keeps, of the second
/// operand's `counts`, those given kept in a bag of `given`: of a batch,
/// a row made only of one kept.
#[inline(never)]
fn keeping(first: Operand, mut counts: Bag, how: Kept, given: Work) -> RowIter {
    let mut given = Bag::new(given);
    match first {
        Operand::Rows(rows) => Box::new(rows.filter_map(move |r| kept(how, (&mut counts, &mut given), r))),
        Operand::Keyed(batches) => Box::new(batches.flat_map(move |b| kept_of(how, (&mut counts, &mut given), b))),
    }
}

/// The rows of the batch `b` the set operation `how` keeps, in order, as
/// `kept` keeps them, then the error of the first it cannot judge, if any;
/// or its error.
fn kept_of(how: Kept, (counts, given): (&mut Bag, &mut Bag), b: Result<Keyed>) -> Vec<Result<Row>> {
    let keyed = match b {
        Ok(keyed) => keyed,
        Err(e) => return vec![Err(e)],
    };
    let mut out = Vec::new();
    for n in 0..keyed.len() {
        match keeps(how, (&mut *counts, &mut *given), keyed.key(n)) {
            Ok(true) => out.push(Ok(keyed.row(n))),
            Ok(false) => {}
            Err(e) => {
                out.push(Err(e));
                break;
            }
        }
    }
    out
}

/// \[DESIGN\] RULES.md D38: the rows of `first` the set operation `how`
/// keeps, of the second operand's counts partitioned in `parts`: `first`
/// partitioned alike, its rows numbered; each pair of partitions dealt with
/// alone (`pair`), those given kept in bags of `given`; and the rows kept
/// merged back by number, in `first`'s order. Never inlined, as `keeping`.
#[inline(never)]
fn partitioned(first: Operand, (parts, how): (Partitioner, Kept), (mut work, given): (Work, Work)) -> RowIter {
    let run = || -> Result<RowIter> {
        let mut lefts = parts.alike(&mut work)?;
        for (n, r) in (0_u64..).zip(first.rows()) {
            let (k, row) = r?;
            lefts.push(&k, &numbered(n, &row))?;
        }
        let mut sources = Vec::new();
        for runs in parts.into_runs().into_iter().zip(lefts.into_runs()) {
            pair(runs, (how, 0), &mut sources, (&mut work, &given))?;
        }
        Ok(Box::new(merge_all(sources, &mut work)?.map(|r| r.map(|(_, row)| row))))
    };
    run().unwrap_or_else(failed)
}

/// The rows of the first operand's partition `left`, numbered, that `how`
/// keeps, of the second operand's counts in partition `right`, both
/// partitioned `depth` times again before, in runs added to `sources`, in
/// order of number: the counts in memory, or, if they outgrow `work`, both
/// partitioned again, while SPILL.PARTITION_DEPTH allows, else in a bag,
/// which spills; the rows given already in a bag of a work area of `given`.
fn pair((right, left): Pair, (how, depth): (Kept, usize), sources: &mut Vec<Records>, (work, given): (&mut Work, &Work)) -> Result<()> {
    let again = depth < work.depth();
    match counted(right.into_records()?.map(|r| r.and_then(|record| uncounted(&record))), work, again)? {
        Counted::Held(mut counts) => {
            let (mut given, mut out) = (Bag::new(given.another()), work.run()?);
            for record in left.into_records()? {
                let (n, row) = unnumbered(&record?, work.types())?;
                if let Some(row) = kept(how, (&mut counts, &mut given), Ok(row)).transpose()? {
                    out.push(&numbered(n, &row))?;
                }
            }
            sources.push(out.into_records()?);
        }
        Counted::Partitioned(parts) => {
            let mut lefts = parts.alike(work)?;
            for record in left.into_records()? {
                let record = record?;
                lefts.push(&key(&unnumbered(&record, work.types())?.1), &record)?;
            }
            for runs in parts.into_runs().into_iter().zip(lefts.into_runs()) {
                pair(runs, (how, depth.saturating_add(1)), &mut *sources, (&mut *work, given))?;
            }
        }
    }
    Ok(())
}

/// A partition of the second operand's counts, and the first's alike.
type Pair = (Box<dyn SpillRun>, Box<dyn SpillRun>);

/// Which rows of the first operand a set operation keeps.
#[derive(Clone, Copy)]
struct Kept {
    intersect: bool,
    all: bool,
}

/// `r`, if the set operation keeps it: `counts` holds the second operand's
/// rows, and `given` those given already. Never inlined into the iterator,
/// as a row is pulled through every level of a plan on the stack (RULES.md
/// R-CF-6).
#[inline(never)]
fn kept(how: Kept, (counts, given): (&mut Bag, &mut Bag), r: Result<Row>) -> Option<Result<Row>> {
    let row = match r {
        Ok(row) => row,
        Err(e) => return Some(Err(e)),
    };
    match keeps(how, (counts, given), &key(&row)) {
        Ok(true) => Some(Ok(row)),
        Ok(false) => None,
        Err(e) => Some(Err(e)),
    }
}

/// Whether the set operation `how` keeps a row of key `k`: `counts` holds
/// the second operand's rows, and `given` those given already.
fn keeps(how: Kept, (counts, given): (&mut Bag, &mut Bag), k: &[u8]) -> Result<bool> {
    if how.all {
        // Each duplicate in the second operand matches one in the first.
        return counts.take(k).map(|matched| matched == how.intersect);
    }
    counts.count(k).and_then(|n| if (n > 0) == how.intersect { given.add_new(k) } else { Ok(false) })
}
