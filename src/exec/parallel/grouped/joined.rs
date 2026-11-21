//! \[DESIGN\] (RULES.md D490) A memoizing groupjoin run in parallel: its
//! build read into a table of keys alone first, each row a group, then its
//! probe's pipeline grouped as any grouping run in parallel by batch is
//! (`waves.rs`), each batch's rows first tagged with the group their keys
//! find in that table, shared by the threads and left as it is
//! (`GroupTable::found`), and grouped by that tag. So each group's set
//! functions take its rows morsel by morsel in order, as a grouping run in
//! parallel takes them, no joined row made; and its groups come in the
//! build's order, or in the order of their first probe rows.

use std::sync::Arc;

use super::{Grouping, Spec, waves};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Batches, Read, read_for};
use crate::exec::groupjoin::{Building, Groups, Tag, given_rows};
use crate::exec::parallel::pipeline::Pipeline;
use crate::exec::parallel::{Held, Limit};
use crate::exec::table::GroupTable;
use crate::exec::{RowIter, broken};
use crate::physical::Plan;
use crate::storage::Row;
use crate::txn::TxId;

/// The rows of the groupjoin `plan`, its probe's morsels grouped on up to
/// `degree` threads; given way where its probe is not read by batch, or
/// past the grant.
///
/// # Errors
///
/// As reading either operand, or a set function, fails.
pub(in crate::exec::parallel) fn group_joined(plan: &Plan, (db, tx): (&Db, TxId), degree: usize) -> Result<Held<RowIter>> {
    let Plan::GroupJoin(g) = plan else { return Err(broken("a groupjoin run in parallel is not one")) };
    let limit = Limit::of(db);
    let Held::Kept(pipeline) = Pipeline::of(&g.probe, (db, tx), &limit)? else { return Ok(Held::GaveWay) };
    let Some(layout) = pipeline.batched.as_ref().map(|by| by.layout.clone()) else { return Ok(Held::GaveWay) };
    let Some(read) = read_for(&[], &g.aggs, &layout) else { return Ok(Held::GaveWay) };
    let seed = db.functions.seed().clone();
    let how = Building { keys: &g.keys, columns: &g.columns, outer: g.outer };
    let keyed = GroupTable::of_keys(g.keys.len(), &seed);
    let (build, build_columns) = crate::exec::batch::probed(g.build.clone(), g.widths.0, (db, tx))?;
    let ceiling = (&seed, db.functions.kernels(), crate::exec::work::Work::ceiling_of(db));
    let grouped = Groups::built(keyed, (build, &build_columns), how, ceiling)?;
    let Some(Groups { table, rows, mut firsts }) = grouped else { return Ok(Held::GaveWay) };
    let places = g.keys.iter().map(|k| layout.iter().position(|&c| c == k.right)).collect::<Option<Vec<_>>>();
    let Some(places) = places else { return Ok(Held::GaveWay) };
    let tag = Arc::new(Tag { table, places, nulls: g.keys.iter().map(|k| k.nulls).collect() });
    // The tag is the column after the batch's own.
    let read = Read { taken: read.taken, groups: vec![layout.len()] };
    let spec = Spec { groups: vec![0], collations: Vec::new(), aggs: g.aggs.clone(), seeds: Vec::new() };
    let grouping = Arc::new(Grouping { spec, distinct: false, read: Some(read), seed, tag: Some(tag) });
    let Held::Kept(runs) = waves::waved(pipeline, &grouping, (db, tx, degree), &limit)? else { return Ok(Held::GaveWay) };
    let results = by_group(runs, &mut firsts)?;
    let made = given_rows((rows, results, firsts), g, (db, tx))?;
    Ok(Held::Kept(limit.keeping(Box::new(made.into_iter().map(Ok)))))
}

/// Each group's results, by its number, of the runs of groups `runs`, each
/// its tag then its set functions' results; and each group's first probe
/// row's number set in `firsts`.
///
/// # Errors
///
/// SQLCODE -910 if a run's group has no tag of a group.
fn by_group(runs: Vec<Vec<(u64, Row)>>, firsts: &mut [u64]) -> Result<Vec<Option<Row>>> {
    let mut results: Vec<Option<Row>> = vec![None; firsts.len()];
    for (first, row) in runs.into_iter().flatten() {
        let mut values = row.into_iter();
        let tag = values.next().and_then(|v| crate::exec::groupjoin::tag_of(&v)).ok_or_else(|| broken("a groupjoin's group of no tag"))?;
        let (Some(slot), Some(at)) = (results.get_mut(tag), firsts.get_mut(tag)) else {
            return Err(broken("a groupjoin's tag past its groups"));
        };
        *slot = Some(values.collect());
        *at = first;
    }
    Ok(results)
}

/// The batches of `batches`, each tagged by `tag` (`Tag::tagged`).
pub(super) fn tagged(tag: &Arc<Tag>, batches: Batches, db: &Db) -> Batches {
    let (tag, db) = (Arc::clone(tag), db.clone());
    Box::new(batches.map(move |kept| kept.map(|kept| tag.tagged(kept, (db.functions.seed(), db.functions.kernels())))))
}
