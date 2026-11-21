//! \[DESIGN\] (RULES.md D490) A memoizing groupjoin run, as Moerkotte and
//! Neumann, "Accelerating Queries with Group-By and Join by Groupjoin"
//! (PVLDB 2011), run one: its build's rows read into the executor's table
//! by their keys, each a group (`groupjoin/groups.rs`); then its probe's
//! batches, each row's keys hashed a vector at a time and found among the
//! groups, each found row taken by its group and its set functions fed by
//! batch, as a grouping by batch feeds them (`batch/groups.rs`), no joined
//! row made. Each group's set functions take its rows in the order the
//! join gave them, so a sum of approximate numbers is the same sum.
//!
//! Its groups come as the grouping's did: in the build's order, or in the
//! order of each group's first probe row. Of a LEFT join a group no row
//! joined is fed one row of nulls, as the join gives it, so COUNT(*) is 1
//! and a null is eliminated from each other set function's values. A build
//! whose groups hold more than MEMORY.WORK_BYTES, or a probe whose set
//! functions batches cannot feed, gives way to the grouping it stands for
//! (RULES.md D38).

mod groups;

use std::sync::Arc;

use super::Runner;
use super::batch::{lanes, probe_layout, probed, read_for};
use super::table::{GroupTable, Key, Seed, hash_rows, hash_values};
use super::work::Work;
use super::{RowIter, broken, execute, failed};
use crate::db::State;
use crate::error::Result;
use crate::logical::AggCall;
use crate::physical::{GroupJoin, Plan};
use crate::storage::sel::nth;
use crate::storage::{Batch, Column, Nulls, Row, Sel};
use crate::txn::TxId;
use crate::value::Value;
use groups::null_key;
pub(in crate::exec) use groups::{Building, Groups, Tag};

/// The rows of the groupjoin `plan`.
#[inline(never)]
pub(super) fn groupjoin(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::GroupJoin(g) = plan else { return failed(broken("a groupjoin is not one")) };
    run(*g, db, tx).unwrap_or_else(failed)
}

/// The rows of `g`, or of the grouping it stands for if it gives way.
fn run(g: GroupJoin, db: &Arc<State>, tx: TxId) -> Result<RowIter> {
    let layout = probe_layout(&g.probe, g.widths.1, db);
    let probe_keys: Vec<usize> = g.keys.iter().map(|k| k.right).collect();
    let Some(read) = read_for(&probe_keys, &g.aggs, &layout) else { return Ok(execute(g.instead, db, tx)) };
    let (seed, runner) = (db.functions.seed().clone(), Runner::new(db, tx));
    let table = GroupTable::new(g.keys.len(), &g.aggs, &seed, usize::MAX);
    let how = Building { keys: &g.keys, columns: &g.columns, outer: g.outer };
    let (build, build_columns) = probed(g.build.clone(), g.widths.0, (db, tx))?;
    let ceiling = (&seed, runner.kernels(), Work::ceiling_of(db));
    let Some(mut groups) = Groups::built(table, (build, &build_columns), how, ceiling)? else {
        return Ok(execute(g.instead, db, tx));
    };
    let (batches, _) = probed(g.probe.clone(), g.widths.1, (db, tx))?;
    let nulls: Vec<bool> = g.keys.iter().map(|k| k.nulls).collect();
    let (mut hashes, mut scratch, mut number) = (Vec::new(), Vec::new(), 0_u64);
    for kept in batches {
        let (batch, rows) = kept?;
        let rows = rows.as_ref().map(Sel::as_slice);
        let count = rows.map_or(batch.rows, <[u32]>::len);
        hash_rows(&batch, (&read.groups, rows), (&seed, runner.kernels()), &mut hashes);
        for (n, &hash) in hashes.iter().enumerate() {
            let r = nth(rows, n);
            if null_key(&batch, (&read.groups, &nulls), r) {
                continue;
            }
            let Some(found) = groups.table.find(hash, Key::Row(&batch, &read.groups, r)) else { continue };
            groups.table.take(found, (n, r));
            let at = number.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
            if let Some(first) = groups.firsts.get_mut(crate::storage::mask::index(found)) {
                *first = (*first).min(at);
            }
        }
        let lanes = lanes(&read.taken, &batch, rows);
        groups.table.feed(&batch, (&read.taken, &lanes), (&mut scratch, &runner))?;
        number = number.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
    let width = groups.table.width();
    let finished = groups.table.finished()?;
    let results = finished.into_iter().map(|(_, row)| Some(row.into_iter().skip(width).collect())).collect();
    let made = given_rows((groups.rows, results, groups.firsts), &g, (db, tx))?;
    Ok(Box::new(made.into_iter().map(Ok)))
}

/// The number of the group the tag `v` names (`Tag::tagged`).
pub(in crate::exec) fn tag_of(v: &Value) -> Option<usize> {
    let Value::Exact(d) = v else { return None };
    usize::try_from(d.units).ok()
}

/// What a groupjoin gathered of its groups: each build row kept, with its
/// group if it has one; each group's results, if it has them; and each
/// group's first probe row's number.
pub(in crate::exec) type Gathered = (Vec<(Option<u32>, Row)>, Vec<Option<Row>>, Vec<u64>);

/// The rows of a groupjoin `g`'s groups: of each build row kept, `rows`,
/// its kept columns then its group's `results`, in the build's order, or
/// in the order of each group's first probe row, `firsts`, if
/// `g.by_first`; a group no row joined, of a first `u64::MAX`, given the
/// results of a row of nulls, of a LEFT join, else left out.
///
/// # Errors
///
/// As a set function fails.
pub(in crate::exec) fn given_rows((rows, mut results, firsts): Gathered, g: &GroupJoin, (db, tx): (&Arc<State>, TxId)) -> Result<Vec<Row>> {
    let empty =
        if g.outer { Some(empty_results((&g.aggs, &g.marked), g.widths.1, (db.functions.seed(), &Runner::new(db, tx)))?) } else { None };
    let mut made = Vec::with_capacity(rows.len());
    for (group, kept) in rows {
        let at = group.map(crate::storage::mask::index);
        let first = at.and_then(|at| firsts.get(at).copied()).unwrap_or(u64::MAX);
        let joined = at.filter(|_| first != u64::MAX).and_then(|at| results.get_mut(at)).and_then(Option::take);
        let Some(values) = joined.or_else(|| empty.clone()) else { continue };
        let mut row = kept;
        row.extend(values);
        made.push((first, row));
    }
    if g.by_first {
        made.sort_by_key(|(first, _)| *first);
    }
    Ok(made.into_iter().map(|(_, row)| row).collect())
}

/// The results of `aggs` over one row of nulls, of a probe whose batches
/// hold `width` vectors: a group of a LEFT join no row joined, as the join
/// gives it one row, null in every column of the probe; of a set function
/// `marked`, over no row, as its marker's filter leaves it none.
///
/// # Errors
///
/// As a set function fails.
fn empty_results((aggs, marked): (&[AggCall], &[bool]), width: usize, (seed, runner): (&Seed, &Runner)) -> Result<Row> {
    let batch = Batch { columns: (0..width).map(|_| Column::Nulls(Nulls::new(1))).collect(), rows: 1 };
    let layout: Vec<usize> = (0..width).collect();
    let mut results = Row::with_capacity(aggs.len());
    for (agg, &unfed) in aggs.iter().zip(marked) {
        let one = std::slice::from_ref(agg);
        let mut table = GroupTable::new(0, one, seed, usize::MAX);
        let g = table.make(hash_values(seed, &[]), Key::Values(&[]), 0);
        if !unfed {
            let read = read_for(&[], one, &layout).ok_or_else(|| broken("a groupjoin's set function over a row of nulls"))?;
            table.take(g, (0, 0));
            let lanes = lanes(&read.taken, &batch, None);
            table.feed(&batch, (&read.taken, &lanes), (&mut Vec::new(), runner))?;
        }
        let finished = table.finished()?.into_iter().next().map(|(_, row)| row);
        results.extend(finished.ok_or_else(|| broken("a groupjoin's group of a row of nulls"))?);
    }
    Ok(results)
}
