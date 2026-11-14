//! [X3.135-1986 §5.22, §5.8] Grouping and set functions: rows partitioned into
//! groups, each set function fed its group's rows. \[DESIGN\] A plug-in's
//! accumulator is opaque, so it is never written out (RULES.md D38): a pass
//! holds groups until they fill the work area, and the rows of groups it has
//! no room for are partitioned by grouping key, numbered by where they came
//! (`partition.rs`), so each partition's groups are made alone; a partition
//! that outgrows the work area too is partitioned again, as many times as
//! SPILL.PARTITION_DEPTH says, then made in passes, each reading again the
//! rows the last had no room for. Each group is numbered by its first row, and
//! the groups merged by number, so they are given in the order their first
//! rows came, as in memory. The values a DISTINCT set function has taken, of
//! every group of a pass, are kept as keys of one bag, which spills to a table
//! once they outgrow it (`exec/work.rs`): the work area split in two, half the
//! groups', half the bag's, so they hold no more than one. The values of the
//! inverse distribution functions of a pass are sorted in a part of their own
//! of what is left, split so again (`group/distribution.rs`).

mod distribution;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use self::distribution::{Pick, Sorted, Sorting, picked_of, picks, seeded};
use super::partition::{Partitioner, merge_all, numbered, unnumbered};
use super::work::{Bag, Work};
use super::{RowIter, Runner, column, eval, eval_cond, failed, warn};
use crate::db::State;
use crate::error::{Result, SqlState};
use crate::functions::Accumulator;
use crate::logical::{AggCall, Cond, Truth};
use crate::spill::{Records, SpillRun, norm_row, row_bytes};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Value;
use crate::value::collation::CollationRef;

/// The bytes a set function's state takes in memory, near enough.
pub(super) const STATE_BYTES: usize = 64;

/// What a grouping groups by and computes.
pub(super) struct Spec {
    /// The grouping columns.
    pub(super) groups: Vec<usize>,
    /// What each groups by, if one is not the default collation.
    pub(super) collations: Vec<Option<CollationRef>>,
    /// The set functions.
    pub(super) aggs: Vec<AggCall>,
    /// [ISO/IEC 9075-2:2023 §7.13 GR2a] The values of groups made before
    /// any row, so they are groups even of no rows.
    pub(super) seeds: Vec<Row>,
}

/// The key of a group of `values`: by their sort keys where `spec` says, so
/// values equal by a collation are of one group, which keeps the first's
/// [ISO/IEC 9075:1992 §8.2 GR3d].
fn group_key(spec: &Spec, values: &Row) -> Vec<u8> {
    if spec.collations.is_empty() {
        return norm_row(values);
    }
    let keyed = |(v, c): (&Value, &Option<CollationRef>)| c.as_ref().map_or_else(|| v.clone(), |c| c.sort_key(v));
    norm_row(&values.iter().zip(&spec.collations).map(keyed).collect::<Row>())
}

/// One group: its grouping values, the set functions fed so far, of its
/// inverse distribution functions what picks their values, and the number
/// of its first row, which orders the output.
struct Group {
    first: u64,
    values: Row,
    states: Vec<Box<dyn Accumulator>>,
    picks: Vec<Option<Pick>>,
}

/// What a pass groups: seeds, each its group's values, then rows, each
/// with its number.
enum Seeded {
    Seed(u64, Row),
    Row(u64, Row),
}

/// What a pass groups, in order.
type Numbered<'a> = Box<dyn Iterator<Item = Result<Seeded>> + 'a>;

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
/// functions, the half of it their bag of values taken holds, and of
/// inverse distribution functions, the part their values are sorted in.
struct Grouper {
    spec: Spec,
    runner: Runner,
    work: Work,
    bag: Option<Work>,
    sorts: Option<Work>,
}

/// What the rows of a pass are fed to beside their groups: the bag of the
/// values DISTINCT set functions took, and the sort of the values of
/// inverse distribution functions.
struct Fed<'a> {
    runner: &'a Runner,
    seen: Bag,
    sorting: Option<Sorting>,
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
    /// it, split in two if a set function is DISTINCT, and what is left in
    /// two again if one is an inverse distribution function.
    fn new(spec: Spec, db: &Arc<State>, tx: TxId) -> Grouper {
        let mut work = Work::of(db, tx);
        let bag = spec.aggs.iter().any(|a| a.distinct).then(|| work.split());
        let sorts = spec.aggs.iter().any(|a| a.distribution.is_some()).then(|| work.split());
        Grouper { spec, runner: Runner::new(db, tx), work, bag, sorts }
    }

    /// A new group of the grouping values `values`, its first row numbered
    /// `first`: `row`, or of a group of no rows none.
    fn group(&self, first: u64, values: Row, row: Option<&Row>) -> Result<Group> {
        let picks = match row {
            Some(row) => picks(&self.spec.aggs, row, &self.runner)?,
            None => picks(&self.spec.aggs, &seeded(&self.spec.aggs, &self.spec.groups, &values), &self.runner)?,
        };
        Ok(Group { first, values, states: fresh(&self.spec), picks })
    }

    /// The groups of `input`, from memory if they fit the work area, else
    /// merged from runs. Without GROUP BY, there is a single group even of
    /// no rows ([X3.135-1986 §5.23 GR1, §5.25 GR1]); and the seeds are
    /// groups too, made first, numbered before the rows, in the first pass,
    /// which holds them whatever else it holds.
    fn grouped(&mut self, input: RowIter) -> Result<RowIter> {
        let mut overflow = Overflow::Partitions(None);
        let seeds = std::mem::take(&mut self.spec.seeds);
        let first = u64::try_from(seeds.len()).unwrap_or(u64::MAX);
        let seeded = (0_u64..).zip(seeds).map(|(n, values)| Ok(Seeded::Seed(n, values)));
        let rows = (first..).zip(input).map(|(n, r)| r.map(|row| Seeded::Row(n, row)));
        let mut held = self.pass(Box::new(seeded.chain(rows)), &mut overflow)?;
        if self.spec.groups.is_empty() && held.is_empty() {
            held.push((0, finished(self.group(0, Vec::new(), None)?, &self.spec.aggs, None)?));
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
            let records = records.map(move |r| r.and_then(|record| unnumbered(&record, &types)).map(|(n, row)| Seeded::Row(n, row)));
            let held = self.pass(Box::new(records), &mut overflow)?;
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
        let seen = Bag::new(self.bag.as_ref().unwrap_or(&self.work).another());
        let mut fed = Fed { runner: &self.runner, seen, sorting: self.sorts.as_ref().map(|w| Sorting::new(w.another())) };
        for r in rows {
            let (n, row) = match r? {
                Seeded::Seed(n, values) => {
                    let key = group_key(&self.spec, &values);
                    bytes = bytes.saturating_add(key.len()).saturating_add(row_bytes(&values));
                    if let Entry::Vacant(vacant) = found.entry(key) {
                        vacant.insert(self.group(n, values, None)?);
                    }
                    continue;
                }
                Seeded::Row(n, row) => (n, row),
            };
            let values: Row = self.spec.groups.iter().map(|&c| column(&row, c).cloned()).collect::<Result<_>>()?;
            let key = group_key(&self.spec, &values);
            if !found.contains_key(&key) {
                if bytes > self.work.limit() {
                    overflow.push(&key, &numbered(n, &row), &mut self.work)?;
                    continue;
                }
                bytes = bytes.saturating_add(key.len()).saturating_add(row_bytes(&values));
                bytes = bytes.saturating_add(self.spec.aggs.len().saturating_mul(STATE_BYTES));
                found.insert(key.clone(), self.group(n, values, Some(&row))?);
            }
            if let Some(group) = found.get_mut(&key) {
                feed(&self.spec.aggs, (group, &key), &row, &mut fed)?;
            }
        }
        let mut found: Vec<Group> = found.into_values().collect();
        found.sort_by_key(|g| g.first);
        let mut sorted = fed.sorting.map(Sorting::sorted).transpose()?;
        found.into_iter().map(|g| (g.first, finished(g, &self.spec.aggs, sorted.as_mut()))).map(|(n, row)| row.map(|r| (n, r))).collect()
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
    spec.aggs.iter().map(|a| a.func.0.accumulator(a.result.clone())).collect()
}

/// Feeds each set function of `group`, of grouping key `key`, its
/// argument of `row`, the values each DISTINCT one took kept in the bag of
/// `fed`, those of each inverse distribution function sorted in its sort.
fn feed(aggs: &[AggCall], (group, key): (&mut Group, &[u8]), row: &Row, fed: &mut Fed<'_>) -> Result<()> {
    let (runner, seen, sorting) = (fed.runner, &mut fed.seen, &mut fed.sorting);
    let Group { first, states, picks, .. } = group;
    for (n, (a, acc)) in aggs.iter().zip(states.iter_mut()).enumerate() {
        // [ISO/IEC 9075-2:2023 §10.9 GR4] a row its filter is not true of
        // is not aggregated, its argument not computed.
        if !admitted(a.filter.as_ref(), row, runner)? {
            continue;
        }
        match &a.arg {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            None => acc.update(&Value::Null)?,
            Some(e) => {
                let v = eval(e, row, runner)?;
                // [ISO/IEC 9075:1992 §6.5 GR1b] a null eliminated warns.
                if v.is_null() {
                    warn(runner, SqlState::NULL_VALUE_ELIMINATED);
                }
                // [ISO/IEC 9075-2:2023 §10.9 GR10f] an inverse distribution
                // function's values not null are sorted.
                if let (Some(d), Some(pick), Some(sorting)) = (&a.distribution, picks.get_mut(n).and_then(Option::as_mut), sorting.as_mut())
                {
                    sorting.push((*first, n), (d.order, pick), v)?;
                    continue;
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

/// [ISO/IEC 9075-2:2023 §10.9 GR4] Whether a set function of the filter
/// `filter` aggregates `row`: the filter true of it, or none written.
pub(super) fn admitted(filter: Option<&Cond>, row: &Row, runner: &Runner) -> Result<bool> {
    match filter {
        None => Ok(true),
        Some(c) => Ok(eval_cond(c, row, runner)? == Truth::True),
    }
}

/// The row of a finished group: its grouping values, then each set
/// function's result, of `aggs`, of an inverse distribution function of
/// its values read from `sorted`.
fn finished(Group { first, values: mut row, states, picks }: Group, aggs: &[AggCall], mut sorted: Option<&mut Sorted>) -> Result<Row> {
    for (n, (acc, call)) in states.into_iter().zip(aggs).enumerate() {
        match picks.get(n).and_then(Option::as_ref) {
            Some(pick) => row.push(picked_of(sorted.as_deref_mut(), (first, n), call, pick)?),
            None => row.push(acc.finish()?),
        }
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

// Unit tests: the set-function argument decision, and groups made before
// any row (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MemoryCatalog;
    use crate::txn::NO_TRANSACTION;
    use crate::value::Decimal;

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 GR2a] a seed is a
    /// group even of no rows, its set functions fed nothing; rows of its
    /// values feed it, no second group made of them; rows of others make
    /// groups after it.
    ///
    /// Decision: a seed's key is or is not that of a row. Each row set
    /// changes the outcome alone: none, one of the seed's key, one of
    /// another key.
    #[test]
    fn seeds_are_groups_even_of_no_rows() {
        let db = Arc::new(State::with_builtins(Box::new(MemoryCatalog::default())));
        let count = db.functions.aggregate("COUNT").unwrap();
        let result = count.0.result_type(None).unwrap();
        let agg = AggCall { func: count, arg: None, distinct: false, result, filter: None, distribution: None, label: "COUNT(*)".into() };
        let int = |n: i128| Value::Exact(Decimal::new(n, 0));
        let grouped = |input: Vec<Row>| {
            let spec = Spec { groups: vec![0], collations: Vec::new(), aggs: vec![agg.clone()], seeds: vec![vec![int(9)]] };
            let rows = grouping(spec, Box::new(input.into_iter().map(Ok)), &db, NO_TRANSACTION);
            rows.map(|r| r.unwrap().iter().map(|v| format!("{v:?}")).collect::<Vec<_>>().join(",")).collect::<Vec<_>>()
        };
        let (nine, five) = (format!("{:?}", int(9)), format!("{:?}", int(5)));
        assert_eq!(grouped(Vec::new()), [format!("{nine},{:?}", int(0))]);
        assert_eq!(grouped(vec![vec![int(9)]]), [format!("{nine},{:?}", int(1))]);
        assert_eq!(grouped(vec![vec![int(5)]]), [format!("{nine},{:?}", int(0)), format!("{five},{:?}", int(1))]);
    }

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
