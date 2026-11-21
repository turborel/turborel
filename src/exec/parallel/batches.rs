//! \[DESIGN\] (RULES.md D389) A pipeline run in parallel read by batch by
//! an operator that is not one, as an outer join's operand: its morsels
//! taken through its steps by batch on up to the degree's threads, as a
//! part's rows are (`parallel.rs`), and their batches given in the order
//! of their morsels, each morsel's in order, so the operator reads them
//! as it would the stream's run alone. The operator then reads the part
//! by batch, not a row at a time, as it reads the plan alone; before, a
//! part so marked read row by row was slower on many threads than its
//! plan alone on one. A pipeline whose morsels are not read by batch
//! gives way to its plan alone.

use std::sync::Arc;

use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Batches, Kept, gathered, rows_of};
use crate::physical::Plan;
use crate::txn::{Stop, TxId};

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
    let limit = Limit::of(db);
    let Held::Kept(pipeline) = Pipeline::of(plan, (db, tx), &limit)? else { return Ok(Held::GaveWay) };
    let Some(shape) = pipeline.batched.as_ref().map(|b| (b.layout.clone(), b.width)) else { return Ok(Held::GaveWay) };
    let (pipeline, db_for) = (Arc::new(pipeline), db.clone());
    let count = pipeline.count();
    let work = Box::new(move |m: usize| -> Result<Vec<Kept>> { pipeline.morsel_kept(m, (&db_for, tx))?.collect() });
    let morsels = Morsels::start(db, tx, (count, degree), work);
    let batches: Batches = Box::new(InOrder { morsels, batches: Vec::new().into_iter(), stop: db.tx.stop_of(tx), ended: false });
    if shape == *layout {
        return Ok(Held::Kept(batches));
    }
    // Batches of another layout than the plan's are made again of its rows.
    Ok(Held::Kept(gathered(rows_of(batches, shape.0, shape.1), &layout.0)))
}

/// Batches given a morsel's at a time, in order, the statement's
/// cancellation looked at as each morsel's are taken up (RULES.md D44).
struct InOrder {
    morsels: Morsels<Vec<Kept>>,
    batches: std::vec::IntoIter<Kept>,
    stop: Option<Arc<Stop>>,
    ended: bool,
}

impl Iterator for InOrder {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Result<Kept>> {
        loop {
            if let Some(kept) = self.batches.next() {
                return Some(Ok(kept));
            }
            if self.ended {
                return None;
            }
            let taken = self.stop.as_ref().map_or(Ok(()), |s| s.check()).and_then(|()| self.morsels.next().transpose());
            match taken {
                Ok(Some(batches)) => self.batches = batches.into_iter(),
                Ok(None) => return None,
                Err(e) => {
                    self.ended = true;
                    return Some(Err(e));
                }
            }
        }
    }
}
