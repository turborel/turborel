//! \[DESIGN\] A pipeline's morsels read by batch (RULES.md D354): each
//! morsel's rows read as the serial scan by batch reads them
//! (`Split::morsel_batches`), taken through the steps of its pipeline by
//! the functions the serial stream applies (`exec/batch/shaped.rs`), a
//! filter judged over the vectors, a projection of their columns, so a
//! worker costs a row what the statement's thread alone costs it, as
//! Leis et al., "Morsel-Driven Parallelism" (2014), run a morsel's
//! pipeline fragment whole on one thread. A pipeline whose every branch
//! is a table's scan whose engine reads by batch, and whose every step is
//! a filter, a projection or a probe the serial stream reads by batch, is
//! read so; any other, as an index scan's, row by row
//! (`parallel/pipeline.rs`).
//! A morsel's batches give the rows, in the order, and raise the error,
//! that its rows would.

use std::sync::Arc;

use super::pipeline::{Branch, Pipeline, Step};
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{
    Batches, Deferred, Shaped, filter_step, outer_step, probe_step, project_step, resolved, rows_of, runtime_step, semi_step, through,
};
use crate::exec::{RowIter, broken};
use crate::txn::TxId;

/// How a pipeline's morsels are read by batch: each branch's steps over
/// its batches, and the columns of the rows the batches of every branch
/// hold at its end, and the width of those rows.
pub(super) struct ByBatch {
    steps: Vec<Vec<Shaped>>,
    /// Of each branch, the columns its morsels give late (RULES.md D366).
    late: Vec<Vec<usize>>,
    pub(super) layout: Vec<usize>,
    pub(super) width: usize,
}

/// How `branches` are read by batch, if each is (as the module's
/// documentation says) and `db` reads by batch (EXECUTOR.BATCHES).
pub(super) fn by_batch(branches: &[Branch], db: &State) -> Option<ByBatch> {
    if !crate::exec::batched(db) {
        return None;
    }
    let (mut steps, mut late) = (Vec::with_capacity(branches.len()), Vec::with_capacity(branches.len()));
    let mut shape: Option<(Vec<usize>, usize)> = None;
    for branch in branches {
        let (shaped, layout, width) = branch_steps(branch)?;
        if shape.as_ref().is_some_and(|(l, w)| (l, *w) != (&layout, width)) {
            return None;
        }
        shape = Some((layout, width));
        steps.push(shaped);
        late.push(late_of(branch, width));
    }
    let (layout, width) = shape?;
    Some(ByBatch { steps, late, layout, width })
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
            Step::Probe(table) => {
                let left: Vec<usize> = table.keys().iter().map(|k| k.left).collect();
                let made;
                (made, layout, width) = probe_step(&table.shared, &left, (&layout, width))?;
                shaped.push(made);
            }
            Step::Outer(table) => {
                let left: Vec<usize> = table.keys().iter().map(|k| k.left).collect();
                let made;
                (made, layout, width) = outer_step(&table.shared, &left, (&layout, width))?;
                shaped.push(made);
            }
            Step::Semi(table, semi) => {
                let join = (table.keys(), semi.cond.as_ref(), semi.anti);
                shaped.push(semi_step(&table.shared, join, (layout.as_slice(), semi.widths))?);
            }
            Step::Runtime(told) => shaped.push(runtime_step(told, &layout)),
            Step::Count(count) => shaped.push(Shaped::Count(Arc::clone(count))),
        }
    }
    Some((shaped, layout, width))
}

/// \[DESIGN\] (RULES.md D361, D366) The columns of `branch`'s scan no step
/// reads before its rows, `width` wide at its end, are made, if a filter
/// drops rows on the way: its morsels give them late, decoded of the rows
/// kept at the pipeline's end (`morsel_kept`), as the serial stream does.
fn late_of(branch: &Branch, width: usize) -> Vec<usize> {
    let Some((scanned, _)) = &branch.scanned else { return Vec::new() };
    let mut deferred = Deferred::all(width);
    for step in branch.steps.iter().rev() {
        deferred = match step {
            Step::Filter(cond) => deferred.filtered(cond),
            Step::Project(exprs) => deferred.projected(exprs),
            Step::Probe(_) | Step::Outer(_) | Step::Semi(..) => Deferred::default(),
            Step::Runtime(told) => deferred.judged(&told.iter().map(|t| t.column).collect::<Vec<_>>()),
            Step::Count(_) => deferred,
        };
    }
    deferred.late_of(scanned)
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
    pub(super) fn morsel_kept(&self, n: usize, (db, tx): (&Db, TxId)) -> Result<Batches> {
        let by = self.batched.as_ref().ok_or_else(|| broken("a pipeline not read by batch"))?;
        let mut at = n;
        for ((branch, steps), late) in self.branches.iter().zip(&by.steps).zip(&by.late) {
            let morsels = branch.split.morsels();
            if at < morsels {
                let read = branch.split.morsel_batches(at, late).ok_or_else(|| broken("a morsel not read by batch"))?;
                let kept: Batches = Box::new(read.map(|b| b.map(|b| (b, None))));
                return Ok(resolved(through(kept, steps, (db, tx)), crate::db::kernels::chosen(db)));
            }
            at = at.saturating_sub(morsels);
        }
        Err(broken("a morsel past a pipeline's last"))
    }

    /// The rows of morsel `n`, read by batch if the pipeline is, else row
    /// by row (`parallel/rows.rs`), in order, each made as it is asked for.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `n` is past its last morsel; a row or a step that
    /// fails is an error item after the rows before it.
    pub(super) fn morsel_rows(self: &Arc<Self>, n: usize, (db, tx): (&Db, TxId), runner: crate::exec::Runner) -> Result<RowIter> {
        let Some(by) = &self.batched else { return super::rows::morsel_rows(self, n, runner) };
        Ok(rows_of(self.morsel_kept(n, (db, tx))?, by.layout.clone(), by.width))
    }
}
