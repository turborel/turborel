//! [ISO/IEC 9075:1992 §7.2, §7.10] The rows of a `<table value
//! constructor>`, and of EXCEPT and INTERSECT; UNION is UNION ALL, an
//! append, under duplicate elimination (`physical.rs`). \[DESIGN\] RULES.md
//! D38: EXCEPT and INTERSECT count the second operand's rows in memory
//! while they fit the work area, else partition both operands alike.
//! Split from exec.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;
use std::sync::Arc;

use super::partition::{Partitioner, merge_all, numbered, unnumbered};
use super::work::{Bag, ENTRY_BYTES, Work};
use super::{RowIter, Runner, broken, eval, execute, failed};
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
    let second = execute(*right, db, tx);
    let how = Kept { intersect: op == SetOp::Intersect, all };
    match counted_rows(second, db, tx) {
        Ok((Counted::Held(counts), work)) => {
            let first = execute(*left, db, tx);
            keeping(first, counts, how, &work)
        }
        Ok((Counted::Partitioned(parts), work)) => {
            let first = execute(*left, db, tx);
            partitioned(first, (parts, how), work)
        }
        Err(e) => failed(e),
    }
}

/// Keys counted: in a bag, or, once they outgrew the work area,
/// partitioned by key, each record a count and a key.
enum Counted {
    Held(Bag),
    Partitioned(Partitioner),
}

/// The rows of `rows`, counted, in a work area of `db`, for the statement
/// of `tx`. Kept apart from `operation`, whose frame is on the stack while
/// its operands are made (RULES.md R-CF-6).
#[inline(never)]
fn counted_rows(rows: RowIter, db: &State, tx: TxId) -> Result<(Counted, Work)> {
    let mut work = Work::of(db, tx);
    let counts = counted(rows.map(|r| r.map(|row| (key(&row), 1))), &mut work, true)?;
    Ok((counts, work))
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
/// operand's `counts`.
#[inline(never)]
fn keeping(first: RowIter, mut counts: Bag, how: Kept, work: &Work) -> RowIter {
    let mut given = Bag::new(work.another());
    Box::new(first.filter_map(move |r| kept(how, (&mut counts, &mut given), r)))
}

/// \[DESIGN\] RULES.md D38: the rows of `first` the set operation `how`
/// keeps, of the second operand's counts partitioned in `parts`: `first`
/// partitioned alike, its rows numbered; each pair of partitions dealt with
/// alone (`pair`); and the rows kept merged back by number, in `first`'s
/// order. Never inlined, as `keeping`.
#[inline(never)]
fn partitioned(first: RowIter, (parts, how): (Partitioner, Kept), mut work: Work) -> RowIter {
    let run = || -> Result<RowIter> {
        let mut lefts = parts.alike(&mut work)?;
        for (n, r) in (0_u64..).zip(first) {
            let row = r?;
            lefts.push(&key(&row), &numbered(n, &row))?;
        }
        let mut sources = Vec::new();
        for runs in parts.into_runs().into_iter().zip(lefts.into_runs()) {
            pair(runs, (how, 0), &mut sources, &mut work)?;
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
/// which spills.
fn pair((right, left): Pair, (how, depth): (Kept, usize), sources: &mut Vec<Records>, work: &mut Work) -> Result<()> {
    let again = depth < work.depth();
    match counted(right.into_records()?.map(|r| r.and_then(|record| uncounted(&record))), work, again)? {
        Counted::Held(mut counts) => {
            let (mut given, mut out) = (Bag::new(work.another()), work.run()?);
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
                pair(runs, (how, depth.saturating_add(1)), &mut *sources, work)?;
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
    let k = key(&row);
    let keep = if how.all {
        // Each duplicate in the second operand matches one in the first.
        counts.take(&k).map(|matched| matched == how.intersect)
    } else {
        counts.count(&k).and_then(|n| if (n > 0) == how.intersect { given.add_new(&k) } else { Ok(false) })
    };
    match keep {
        Ok(true) => Some(Ok(row)),
        Ok(false) => None,
        Err(e) => Some(Err(e)),
    }
}
