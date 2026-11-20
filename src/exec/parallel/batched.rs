//! \[DESIGN\] A pipeline's morsels read by batch (RULES.md D354): each
//! morsel's rows read as the serial scan by batch reads them
//! (`Split::morsel_batches`), taken through the steps of its pipeline by
//! the functions the serial stream applies (`exec/batch/shaped.rs`), a
//! filter judged over the vectors, a projection of their columns, so a
//! worker costs a row what the statement's thread alone costs it, as
//! Leis et al., "Morsel-Driven Parallelism" (2014), run a morsel's
//! pipeline fragment whole on one thread. A pipeline whose every branch
//! is a table's scan whose engine reads by batch, and whose every step is
//! a filter or a projection the serial stream reads by batch, is read so;
//! any other, as a hash join's probe, row by row (`parallel/pipeline.rs`).
//! A morsel's batches give the rows, in the order, and raise the error,
//! that its rows would.

use std::sync::Arc;

use super::pipeline::{Branch, Pipeline, Step};
use crate::db::State;
use crate::error::Result;
use crate::exec::batch::{Batches, Shaped, filter_step, project_step, rows_of, through};
use crate::exec::broken;
use crate::storage::Row;
use crate::txn::TxId;

/// How a pipeline's morsels are read by batch: each branch's steps over
/// its batches, and the columns of the rows the batches of every branch
/// hold at its end, and the width of those rows.
pub(super) struct ByBatch {
    steps: Vec<Vec<Shaped>>,
    pub(super) layout: Vec<usize>,
    pub(super) width: usize,
}

/// How `branches` are read by batch, if each is (as the module's
/// documentation says) and `db` reads by batch (EXECUTOR.BATCHES).
pub(super) fn by_batch(branches: &[Branch], db: &State) -> Option<ByBatch> {
    if !crate::exec::batched(db) {
        return None;
    }
    let mut steps = Vec::with_capacity(branches.len());
    let mut shape: Option<(Vec<usize>, usize)> = None;
    for branch in branches {
        let (shaped, layout, width) = branch_steps(branch)?;
        if shape.as_ref().is_some_and(|(l, w)| (l, *w) != (&layout, width)) {
            return None;
        }
        shape = Some((layout, width));
        steps.push(shaped);
    }
    let (layout, width) = shape?;
    Some(ByBatch { steps, layout, width })
}

/// The steps of `branch` over its batches, and the columns its batches
/// hold at its end and their rows' width, if each step is read by batch.
fn branch_steps(branch: &Branch) -> Option<(Vec<Shaped>, Vec<usize>, usize)> {
    let (mut layout, mut width) = branch.scanned.clone()?;
    let mut shaped = Vec::with_capacity(branch.steps.len());
    for step in &branch.steps {
        match step {
            Step::Filter(cond) => shaped.push(filter_step(cond, &layout)),
            Step::Project(exprs) => {
                let (made, after) = project_step(exprs, &layout)?;
                shaped.push(made);
                (layout, width) = (after, exprs.len());
            }
            Step::Probe(_) => return None,
        }
    }
    Some((shaped, layout, width))
}

impl Pipeline {
    /// The batches of morsel `n`, each with the rows its steps kept, of a
    /// pipeline read by batch, its expressions evaluated for the statement
    /// of `tx`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the pipeline is not read by batch, or `n` is past its
    /// last morsel.
    pub(super) fn morsel_kept(&self, n: usize, (db, tx): (&Arc<State>, TxId)) -> Result<Batches> {
        let by = self.batched.as_ref().ok_or_else(|| broken("a pipeline not read by batch"))?;
        let mut at = n;
        for (branch, steps) in self.branches.iter().zip(&by.steps) {
            let morsels = branch.split.morsels();
            if at < morsels {
                let read = branch.split.morsel_batches(at).ok_or_else(|| broken("a morsel not read by batch"))?;
                let kept: Batches = Box::new(read.map(|b| b.map(|b| (b, None))));
                return Ok(through(kept, steps, (db, tx)));
            }
            at = at.saturating_sub(morsels);
        }
        Err(broken("a morsel past a pipeline's last"))
    }

    /// The rows of morsel `n`, read by batch if the pipeline is, else row
    /// by row (`Pipeline::morsel`), in order.
    ///
    /// # Errors
    ///
    /// As reading a row or a step fails, the rows before it given up.
    pub(super) fn morsel_rows(&self, n: usize, (db, tx): (&Arc<State>, TxId), runner: &crate::exec::Runner) -> Result<Vec<Row>> {
        let Some(by) = &self.batched else { return self.morsel(n, runner) };
        rows_of(self.morsel_kept(n, (db, tx))?, by.layout.clone(), by.width).collect()
    }
}
