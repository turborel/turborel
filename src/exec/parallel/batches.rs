//! \[DESIGN\] (RULES.md D389) A pipeline run in parallel read by batch by
//! an operator that is not one, as an outer join's operand: its morsels
//! taken through its steps by batch on up to the degree's threads, as a
//! part's rows are (`parallel.rs`), and their batches given in the order
//! of their morsels, each morsel's in order, so the operator reads them
//! as it would the stream's run alone, each morsel's gathered within the
//! part's grant, or read by the operator as it is given if it outgrows it
//! (`parallel/charged.rs`, RULES.md D1150). The operator then reads the part
//! by batch, not a row at a time, as it reads the plan alone; before, a
//! part so marked read row by row was slower on many threads than its
//! plan alone on one. A pipeline whose morsels are not read by batch
//! gives way to its plan alone.

use std::sync::Arc;

use super::charged::{Ahead, Flow, whole_batches};
use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Batches, Kept, gathered, rows_of};
use crate::physical::Plan;
use crate::settings::EXECUTOR_MORSEL_ROWS;
use crate::txn::TxId;

/// The batches of the pipeline `plan` run on up to `degree` threads, each
/// of whose vectors holds the column of its rows at its place in
/// `layout`; given way if its morsels are not read by batch.
///
/// # Errors
///
/// As splitting the pipeline or building a hash join's operand fails.
pub(in crate::exec) fn streamed_batches(
    plan: &Plan,
    (db, tx): (&Db, TxId),
    degree: usize,
    layout: &(Vec<usize>, usize),
) -> Result<Held<Batches>> {
    let (limit, ahead) = (Limit::of(db), Ahead::of(degree, db.settings.count(EXECUTOR_MORSEL_ROWS)));
    let Held::Kept(pipeline) = Pipeline::of(plan, (db, tx), &limit)? else { return Ok(Held::GaveWay) };
    let Some(shape) = pipeline.batched.as_ref().map(|b| (b.layout.clone(), b.width)) else { return Ok(Held::GaveWay) };
    let (pipeline, db_for, within) = (Arc::new(pipeline), db.clone(), (limit.clone(), ahead.clone()));
    let count = pipeline.count();
    let work = Box::new(move |m: usize| whole_batches(pipeline.morsel_kept(m, (&db_for, tx))?, (&within.0, &within.1)));
    let morsels = Morsels::start(db, tx, (count, degree), work);
    let batches: Batches = Box::new(Flow::of_batches(morsels, (&limit, &ahead), db.tx.stop_of(tx)));
    if shape == *layout {
        return Ok(Held::Kept(batches));
    }
    // Batches of another layout than the plan's are made again of its rows.
    Ok(Held::Kept(gathered(rows_of(batches, shape.0, shape.1), &layout.0)))
}
