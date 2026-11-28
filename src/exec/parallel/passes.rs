//! \[DESIGN\] (RULES.md D1240) The passes of a pipeline over a hash join
//! whose built operand was put in parts past the grant
//! (`probe/spilled.rs`). The pipeline's morsels are those of each pass in
//! turn, its own first, then each pass's numbered on after them, so what
//! reads the pipeline, a grouping or a gathering of its rows, reads every
//! pass as more morsels, unchanged. A pass's morsels are the pipeline's
//! own scans read again, judged by the same conditions, zones and runtime
//! filters, those of the whole operand (RULES.md D369), and a step before
//! the join's probe keeps the rows whose keys fall in its cells
//! (`Keep`), so each probing row is probed in one pass alone and a LEFT
//! join or an antijoin gives each once.
//!
//! A pass's table is made when its first morsel is asked for, by the
//! threads of the operand's part, and let go once its last is. The
//! morsels are taken in the order of their numbers, so a pass's last is
//! taken before the next pass's first, and no more than the first pass's
//! table, which the pipeline's own steps hold, and two others are held at
//! once, each of no more than a third of the grant (`PASS_SHARE`). A
//! pass's rows are kept, in memory up to their lists' shares and in the
//! spill past them, until the pipeline is let go, so a morsel read again
//! after its pass was let go, as a gathering past its grant reads one
//! again (RULES.md D1150), finds the pass's table made again.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use super::build::Crew;
use super::pipeline::{Branch, Pipeline, Step, copied};
use super::probe::Table;
use super::probe::spilled::{Keep, Pass, Passed, table_of};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::Keeps;
use crate::physical::JoinKey;
use crate::storage::{Batch, Sel};
use crate::txn::TxId;

/// A pass past the first: its rows, kept so a morsel read again after
/// its pass was let go is read of its table made again, and its pipeline
/// while held.
struct Slot {
    pass: Pass,
    made: Option<Arc<Pipeline>>,
}

/// The passes past the first of a pipeline over a hash join built in
/// parts.
pub(super) struct Passes {
    db: Db,
    tx: TxId,
    degree: usize,
    /// The morsels of a pass.
    base: usize,
    shape: (Vec<usize>, usize),
    keys: Vec<JoinKey>,
    /// The first pass's step and table, as the pipeline's steps hold them.
    first: (Arc<Keep>, Arc<Table>),
    slots: Mutex<Vec<Slot>>,
    /// Of each pass past the first, its morsels asked for.
    asked: Vec<AtomicUsize>,
}

impl Passes {
    /// The passes of `passed`, of a pipeline of `base` morsels a pass, run
    /// for the statement of `tx`.
    pub(super) fn of(passed: Passed, (db, tx): (&Db, TxId), base: usize) -> Passes {
        let Passed { table, keep, rest, shape, degree } = passed;
        let asked = rest.iter().map(|_| AtomicUsize::new(0)).collect();
        let keys = table.keys().to_vec();
        Passes {
            db: db.clone(),
            tx,
            degree,
            base,
            shape,
            keys,
            first: (keep, table),
            slots: Mutex::new(rest.into_iter().map(|pass| Slot { pass, made: None }).collect()),
            asked,
        }
    }

    /// How many passes there are, the first among them.
    pub(super) fn count(&self) -> usize {
        self.asked.len().saturating_add(1)
    }

    /// Of morsel `n` of `template`, the pipeline whose passes these are, the
    /// pipeline of its pass and its number there; none of the first pass's,
    /// `template`'s own.
    ///
    /// # Errors
    ///
    /// As making a pass's table fails.
    pub(super) fn of_morsel(&self, template: &Pipeline, n: usize) -> Result<Option<(Arc<Pipeline>, usize)>> {
        let (pass, m) = (n.checked_div(self.base).unwrap_or(0), n.checked_rem(self.base).unwrap_or(n));
        let Some(at) = pass.checked_sub(1) else { return Ok(None) };
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = slots.get_mut(at).ok_or_else(|| crate::exec::broken("a morsel past a pipeline's last pass"))?;
        let made = match slot.made.take() {
            Some(made) => made,
            None => self.made(template, &mut slot.pass)?,
        };
        let asked = self.asked.get(at).map_or(self.base, |a| a.fetch_add(1, Ordering::AcqRel).saturating_add(1));
        if asked < self.base {
            slot.made = Some(Arc::clone(&made));
        }
        Ok(Some((made, m)))
    }

    /// The pipeline of the pass `pass`: `template`'s branches, their scans
    /// shared, the first pass's step and table in their steps those of
    /// this pass, its table made of its rows.
    fn made(&self, template: &Pipeline, pass: &mut Pass) -> Result<Arc<Pipeline>> {
        let crew = Crew { db: &self.db, tx: self.tx, degree: self.degree };
        let table = table_of(&mut pass.lists, (&self.shape, &self.keys), crew, None)?;
        let keep = Arc::new(self.first.0.of_pass(pass.mask.clone()));
        let branches = template
            .branches
            .iter()
            .map(|b| Branch {
                split: Arc::clone(&b.split),
                steps: b.steps.iter().map(|s| self.swapped(s, (&keep, &table))).collect(),
                scanned: b.scanned.clone(),
                vets: b.vets,
            })
            .collect::<Vec<_>>();
        let batched =
            super::batched::by_batch(&branches, &self.db).zip(template.batched.as_ref()).map(|(by, first)| by.late_of_first(first));
        Ok(Arc::new(Pipeline { branches, batched, passes: None }))
    }

    /// `step`, of the first pass, as it is in the pass of `keep` and
    /// `table`: the first pass's step and table those of this one.
    fn swapped(&self, step: &Step, (keep, table): (&Arc<Keep>, &Arc<Table>)) -> Step {
        let (first_keep, first_table) = &self.first;
        let of = |t: &Arc<Table>| Arc::clone(if Arc::ptr_eq(t, first_table) { table } else { t });
        match step {
            Step::Kept(k) => Step::Kept(Arc::clone(if Arc::ptr_eq(k, first_keep) { keep } else { k })),
            Step::Probe(t) => Step::Probe(of(t)),
            Step::Outer(t) => Step::Outer(of(t)),
            Step::Semi(t, semi) => Step::Semi(of(t), Arc::clone(semi)),
            Step::Filter(_) | Step::Project(_) | Step::Runtime(_) | Step::Count(_) => copied(step),
        }
    }
}

/// A `Keep` over batches whose probing keys are the vectors at `places`.
pub(super) struct Placed {
    pub(super) keep: Arc<Keep>,
    pub(super) places: Vec<usize>,
}

impl Keeps for Placed {
    fn keeps(&self, batch: &Batch, rows: Option<Sel>) -> Result<Sel> {
        self.keep.kept(batch, rows, &self.places)
    }
}

/// Of morsel `n` of `pipeline`, the pipeline of its pass and its number
/// there, if it is of a pass past the first.
///
/// # Errors
///
/// As `Passes::of_morsel`.
pub(super) fn passed(pipeline: &Pipeline, n: usize) -> Result<Option<(Arc<Pipeline>, usize)>> {
    match &pipeline.passes {
        Some(passes) => passes.of_morsel(pipeline, n),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests;
