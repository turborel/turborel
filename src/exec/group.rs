//! [X3.135-1986 §5.22, §5.8] Grouping and set functions: rows partitioned
//! into groups, each set function fed its group's rows. \[DESIGN\] A plug-in's
//! accumulator is opaque, so it is never written out (RULES.md D38): a pass
//! holds groups until they fill the work area, and the rows of groups it
//! has no room for are partitioned by grouping key, numbered by where they
//! came (`partition.rs`), so each partition's groups are made alone; a
//! partition that outgrows the work area too is partitioned again, as many
//! times as SPILL.PARTITION_DEPTH says, then made in passes, each reading
//! again the rows the last had no room for. Each group is numbered by its
//! first row, and the groups merged by number, so they are given in the
//! order their first rows came, as in memory. The values a DISTINCT set
//! function has taken, of every group of a pass, are kept as keys of one
//! bag, which spills to a table once they outgrow it (`exec/work.rs`): the
//! work area split in two, half the groups', half the bag's, so they hold
//! no more than one. Split from exec.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;
use std::sync::Arc;

use super::partition::{Partitioner, merge_all, numbered, unnumbered};
use super::work::{Bag, Work};
use super::{RowIter, Runner, column, eval, failed, warn};
use crate::db::State;
use crate::error::{Result, SqlState};
use crate::functions::Accumulator;
use crate::logical::AggCall;
use crate::spill::{Records, SpillRun, norm_row, row_bytes};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Value;

/// The bytes a set function's state takes in memory, near enough.
pub(super) const STATE_BYTES: usize = 64;

/// What a grouping groups by and computes.
pub(super) struct Spec {
    /// The grouping columns.
    pub(super) groups: Vec<usize>,
    /// The set functions.
    pub(super) aggs: Vec<AggCall>,
}

/// One group: its grouping values, the set functions fed so far, and the
/// number of its first row, which orders the output.
struct Group {
    first: u64,
    values: Row,
    states: Vec<Box<dyn Accumulator>>,
}

/// Rows, each with its number.
type Numbered<'a> = Box<dyn Iterator<Item = Result<(u64, Row)>> + 'a>;

/// Groups finished, each with the number of its first row, in that order.
pub(super) type Finished = Vec<(u64, Row)>;

/// [X3.135-1986 §5.22 GR2] The groups of `input` under `spec`: the rows
/// whose grouping values are identical, nulls included per
/// [X3.135-1986 §5.11 GR7], each the grouping values, then one result per
/// set function, in the order their first rows came, in a work area of
/// `db`, read as `tx` sees it. They are made now, as grouping must see
/// every row. Never inlined into the caller, whose frame is on the stack
/// while its input is made (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn grouping(spec: Spec, input: RowIter, db: &Arc<State>, tx: TxId) -> RowIter {
    Grouper::new(spec, db, tx).grouped(input).unwrap_or_else(failed)
}

/// A grouping being made: in its work area, and, of DISTINCT set
/// functions, the half of it their bag of values taken holds.
struct Grouper {
    spec: Spec,
    runner: Runner,
    work: Work,
    bag: Option<Work>,
}

/// Where a pass writes the rows of the groups it has no room for: to
/// partitions, or to one run, for the next pass; each made when first
/// needed.
enum Overflow {
    Partitions(Option<Partitioner>),
    Run(Option<Box<dyn SpillRun>>),
}

impl Overflow {
    /// Writes `record`, of grouping key `key`, in `work`.
    fn push(&mut self, key: &[u8], record: &[u8], work: &mut Work) -> Result<()> {
        match self {
            Overflow::Partitions(parts) => {
                let parts = match parts {
                    Some(parts) => parts,
                    None => parts.insert(Partitioner::of(work)?),
                };
                parts.push(key, record)
            }
            Overflow::Run(run) => {
                let run = match run {
                    Some(run) => run,
                    None => run.insert(work.run()?),
                };
                run.push(record)
            }
        }
    }
}

impl Grouper {
    /// A grouping under `spec`, in a work area of `db`, read as `tx` sees
    /// it, split in two if a set function is DISTINCT.
    fn new(spec: Spec, db: &Arc<State>, tx: TxId) -> Grouper {
        let mut work = Work::of(db, tx);
        let bag = spec.aggs.iter().any(|a| a.distinct).then(|| work.split());
        Grouper { spec, runner: Runner::new(db, tx), work, bag }
    }

    /// The groups of `input`, from memory if they fit the work area, else
    /// merged from runs. Without GROUP BY, there is a single group even of
    /// no rows ([X3.135-1986 §5.23 GR1, §5.25 GR1]).
    fn grouped(&mut self, input: RowIter) -> Result<RowIter> {
        let mut overflow = Overflow::Partitions(None);
        let mut held = self.pass(Box::new((0_u64..).zip(input).map(|(n, r)| r.map(|row| (n, row)))), &mut overflow)?;
        if self.spec.groups.is_empty() && held.is_empty() {
            held.push((0, finished(Group { first: 0, values: Vec::new(), states: fresh(&self.spec) })?));
        }
        let Overflow::Partitions(Some(parts)) = overflow else { return Ok(Box::new(held.into_iter().map(|(_, row)| Ok(row)))) };
        let mut sources = vec![self.written(held)?];
        for run in parts.into_runs() {
            self.partition(run.into_records()?, (0, &mut sources))?;
        }
        Ok(Box::new(merge_all(sources, &mut self.work)?.map(|r| r.map(|(_, row)| row))))
    }

    /// The groups of the numbered rows `partition`, partitioned `depth`
    /// times again before, each pass's groups a run added to `sources`.
    fn partition(&mut self, partition: Records, (depth, sources): (usize, &mut Vec<Records>)) -> Result<()> {
        let mut next = Some(partition);
        while let Some(records) = next.take() {
            let mut overflow = if depth < self.work.depth() { Overflow::Partitions(None) } else { Overflow::Run(None) };
            let types = self.work.types().clone();
            let held = self.pass(Box::new(records.map(move |r| r.and_then(|record| unnumbered(&record, &types)))), &mut overflow)?;
            sources.push(self.written(held)?);
            match overflow {
                Overflow::Partitions(Some(parts)) => {
                    for run in parts.into_runs() {
                        self.partition(run.into_records()?, (depth.saturating_add(1), &mut *sources))?;
                    }
                }
                Overflow::Run(Some(run)) => next = Some(run.into_records()?),
                Overflow::Partitions(None) | Overflow::Run(None) => {}
            }
        }
        Ok(())
    }

    /// Groups the numbered rows `rows`, as many groups as the work area
    /// holds, the rows of the others written to `overflow`.
    fn pass(&mut self, rows: Numbered<'_>, overflow: &mut Overflow) -> Result<Finished> {
        let (mut found, mut bytes) = (HashMap::<Vec<u8>, Group>::new(), 0_usize);
        let mut seen = Bag::new(self.bag.as_ref().unwrap_or(&self.work).another());
        for r in rows {
            let (n, row) = r?;
            let values: Row = self.spec.groups.iter().map(|&c| column(&row, c).cloned()).collect::<Result<_>>()?;
            let key = norm_row(&values);
            if !found.contains_key(&key) {
                if bytes > self.work.limit() {
                    overflow.push(&key, &numbered(n, &row), &mut self.work)?;
                    continue;
                }
                bytes = bytes.saturating_add(key.len()).saturating_add(row_bytes(&values));
                bytes = bytes.saturating_add(self.spec.aggs.len().saturating_mul(STATE_BYTES));
                found.insert(key.clone(), Group { first: n, values, states: fresh(&self.spec) });
            }
            if let Some(group) = found.get_mut(&key) {
                feed(&self.spec.aggs, (group, &key), &row, (&self.runner, &mut seen))?;
            }
        }
        let mut found: Vec<Group> = found.into_values().collect();
        found.sort_by_key(|g| g.first);
        found.into_iter().map(|g| (g.first, finished(g))).map(|(n, row)| row.map(|r| (n, r))).collect()
    }

    /// Groups `held` in a run of the work area, read from the first.
    fn written(&mut self, held: Finished) -> Result<Records> {
        let mut run = self.work.run()?;
        for (n, row) in held {
            run.push(&numbered(n, &row))?;
        }
        run.into_records()
    }
}

/// The set functions of a new group, fed nothing.
fn fresh(spec: &Spec) -> Vec<Box<dyn Accumulator>> {
    spec.aggs.iter().map(|a| a.func.0.accumulator(a.result)).collect()
}

/// Feeds each set function of `group`, of grouping key `key`, its
/// argument of `row`, the values each DISTINCT one took kept in `seen`.
fn feed(aggs: &[AggCall], (group, key): (&mut Group, &[u8]), row: &Row, (runner, seen): (&Runner, &mut Bag)) -> Result<()> {
    for (n, (a, acc)) in aggs.iter().zip(group.states.iter_mut()).enumerate() {
        match &a.arg {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            None => acc.update(&Value::Null)?,
            Some(e) => {
                let v = eval(e, row, runner)?;
                // [ISO/IEC 9075:1992 §6.5 GR1b] a null eliminated warns.
                if v.is_null() {
                    warn(runner, SqlState::NULL_VALUE_ELIMINATED);
                }
                // The group's key, the function's place, and the value: one
                // key for each value each function of each group took.
                let taken = || {
                    seen.add_new(&[key, &u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes(), &norm_row(std::slice::from_ref(&v))].concat())
                };
                if joins_argument(a.distinct, &v, taken)? {
                    acc.update(&v)?;
                }
            }
        }
    }
    Ok(())
}

/// The row of a finished group: its grouping values, then each set
/// function's result.
fn finished(Group { values: mut row, states, .. }: Group) -> Result<Row> {
    for acc in states {
        row.push(acc.finish()?);
    }
    Ok(row)
}

/// [X3.135-1986 §5.8 GR1, GR2] Whether `v` joins the argument of a set function:
/// a null never does, and for DISTINCT each value joins only once (`first`
/// tells whether this is its first time, and keeps it).
fn joins_argument(distinct: bool, v: &Value, first: impl FnOnce() -> Result<bool>) -> Result<bool> {
    if v.is_null() {
        return Ok(false);
    }
    if !distinct {
        return Ok(true);
    }
    first()
}

// Unit tests: the set-function argument decision (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    /// Verifies: [X3.135-1986 §5.8 GR1, GR2] nulls never join a set function's
    /// argument; without DISTINCT every other value does, with DISTINCT once.
    ///
    /// Decisions `v.is_null()` then `!distinct`. Each changes the outcome alone:
    /// null vs value (with and without DISTINCT); DISTINCT vs not, on a repeat.
    #[test]
    fn values_join_the_argument_once_per_distinct() {
        let one = Value::Exact(Decimal::new(1, 0));
        let mut seen = std::collections::HashSet::new();
        let mut joins = |distinct: bool, v: &Value| joins_argument(distinct, v, || Ok(seen.insert(v.norm()))).unwrap();
        assert!(!joins(false, &Value::Null));
        assert!(!joins(true, &Value::Null));
        assert!(joins(false, &one));
        assert!(joins(false, &one)); // a repeat still joins without DISTINCT
        assert!(joins(true, &one)); // first time with DISTINCT
        assert!(!joins(true, &one)); // a repeat does not
    }
}
