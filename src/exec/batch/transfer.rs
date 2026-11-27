//! \[DESIGN\] (RULES.md D665, D668) A phase of predicate transfer run
//! (`physical/transfer.rs`): the chain's leaves taken out of it; each
//! step's leaf read, told the filters made for it so far, its batches
//! held within the work area, each with the rows those filters kept; a
//! Bloom filter and a range of the keys of a column of its rows kept made
//! (`RuntimeFilter`, RULES.md D369), and told the leaf the step judges.
//! A leaf already held is judged by the filters made for it since, as its
//! held batches are read again. Once every step is taken, each leaf goes
//! back into the chain held, or, if it was not, told its filters, and the
//! chain runs as planned over them (`held.rs`).
//!
//! A leaf past what is left of the work area is read again in the chain,
//! told every filter made for it, as Robust Predicate Transfer streams its
//! largest relation: its filter is made of its key's column alone while
//! it is read. A filter that kept more than nine in ten of the rows of the
//! first batches it judged stops judging (`RuntimeFilter::judging`), and
//! the step back along its edge is not taken, as RPT+ drops a transfer
//! that does not pay (Qiao, Boncz and Zhang, PVLDB 2026). The answers do
//! not change: a filter drops only rows no row of the chain holds.

mod held;

use std::sync::Arc;

pub use held::HeldLeaf;

use super::joined::rows_of;
use super::late::{Deferred, resolved};
use super::runtime::{self, Told};
use super::stream::{self, Batches, Kept};
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::table::{Hashing, KeyHashes, RuntimeFilter};
use crate::exec::work::Work;
use crate::exec::{RowIter, broken, failed};
use crate::physical::{Phase, Plan, Transfer, leaves_mut};
use crate::storage::Batch;
use crate::storage::sel::places_of;
use crate::txn::TxId;

/// A leaf as the phase reads it.
struct Slot {
    plan: Plan,
    layout: (Vec<usize>, usize),
    /// Its batches, once held.
    kept: Option<Vec<Kept>>,
    /// Whether it was read and could not be held.
    past: bool,
    /// The filters made for it, each with the leaf it is made of: those it
    /// was judged by, and those not yet.
    judged: Vec<(usize, Told)>,
    waiting: Vec<(usize, Told)>,
}

/// The batches of `plan`, a phase or a held leaf, its chain's columns
/// `deferred` leaves unread given late where it can (RULES.md D361).
///
/// # Errors
///
/// A leaf's errors; SQLCODE -910 of a held leaf read twice.
pub(super) fn batches(plan: Plan, (db, tx): (&Db, TxId), deferred: &Deferred) -> Result<Batches> {
    let Plan::Transfer(t) = plan else { return Err(broken("a predicate transfer is not one")) };
    match *t {
        Transfer::Held(leaf) => leaf.batches((db, tx)),
        Transfer::Phase(phase) => {
            let (chain, work) = ran(*phase, (db, tx))?;
            let batches = stream::batches_with(chain, (db, tx), deferred)?;
            Ok(Box::new(Holding { batches, _work: work }))
        }
    }
}

/// The chain's batches, and the work area its held leaves are charged to,
/// which lasts as long as they are read.
struct Holding {
    batches: Batches,
    _work: Work,
}

impl Iterator for Holding {
    type Item = Result<Kept>;
    fn next(&mut self) -> Option<Result<Kept>> {
        self.batches.next()
    }
}

/// The rows of `plan`, a phase or a held leaf, each of its width.
pub(in crate::exec) fn rows(plan: Plan, db: &Db, tx: TxId) -> RowIter {
    let Some((columns, width)) = stream::layout(&plan, db) else { return failed(broken("a predicate transfer gives no batches")) };
    match batches(plan, (db, tx), &Deferred::default()) {
        Ok(batches) => rows_of(batches, columns, width),
        Err(e) => failed(e),
    }
}

/// The columns of `plan`'s rows its batches hold and their width, if it
/// gives batches (`stream::layout`), for the planner to find a chain's
/// columns by.
pub(crate) fn batch_layout(plan: &Plan, db: &State) -> Option<(Vec<usize>, usize)> {
    stream::layout(plan, db)
}

/// Whether a phase of predicate transfer may run over the chain `plan`:
/// it gives batches, every table it reads reads by batch, and its rows
/// are asked for in no order a held leaf would not keep (`joined::in_order`).
pub(crate) fn transferable(plan: &Plan, db: &State) -> bool {
    stream::layout(plan, db).is_some() && stream::reads_by_batch(plan, db) && !super::joined::ordered(plan)
}

/// The layout of `t`'s batches: its chain's, or its held leaf's.
pub(super) fn layout(t: &Transfer, db: &State) -> Option<(Vec<usize>, usize)> {
    match t {
        Transfer::Phase(p) => stream::layout(&p.input, db),
        Transfer::Held(leaf) => Some(leaf.layout()),
    }
}

/// `phase` run: its chain, each leaf held or told its filters, and the
/// work area its leaves are held in.
///
/// # Errors
///
/// As `batches`.
fn ran(phase: Phase, (db, tx): (&Db, TxId)) -> Result<(Plan, Work)> {
    let Phase { mut input, steps, .. } = phase;
    let mut slots: Vec<Slot> = Vec::new();
    leaves_mut(&mut input, &mut |_, leaf| {
        let plan = std::mem::replace(leaf, Plan::Values { rows: Vec::new(), detail: String::new() });
        let layout = stream::layout(&plan, db).unwrap_or_default();
        slots.push(Slot { plan, layout, kept: None, past: false, judged: Vec::new(), waiting: Vec::new() });
    });
    let work = Work::of(db, tx);
    let mut room = work.limit();
    let mut stopped: Vec<(usize, usize)> = Vec::new();
    for step in steps {
        if stopped.contains(&(step.to.0, step.from.0)) {
            continue;
        }
        let from = slots.get_mut(step.from.0).ok_or_else(|| broken("a step of a leaf the chain lacks"))?;
        let filter = from.filter((step.from.1, step.nulls), (db, tx), &mut room)?;
        stopped.extend(from.judged.iter().filter(|(_, t)| !t.filter.judging()).map(|(of, _)| (*of, step.from.0)));
        let to = slots.get_mut(step.to.0).ok_or_else(|| broken("a step to a leaf the chain lacks"))?;
        if let Some(filter) = filter {
            to.waiting.push((step.from.0, Told::of(filter, step.to.1)));
        }
    }
    let mut back: Vec<Option<Plan>> = slots.into_iter().map(|s| s.finished(db).map(Some)).collect::<Result<_>>()?;
    leaves_mut(&mut input, &mut |n, leaf| {
        if let Some(plan) = back.get_mut(n).and_then(Option::take) {
            *leaf = plan;
        }
    });
    Ok((input, work))
}

impl Slot {
    /// The filter of the keys of column `column` of its rows kept, whose
    /// nulls match as `nulls` says: of its batches held, judged by the
    /// filters made for it since; else read, told every one, and held
    /// within `room`, which they then take, or its column's alone.
    ///
    /// # Errors
    ///
    /// Its plan's errors; SQLCODE -910 of a column its batches lack.
    fn filter(&mut self, (column, nulls): (usize, bool), (db, tx): (&Db, TxId), room: &mut usize) -> Result<Option<Arc<RuntimeFilter>>> {
        let at = self.layout.0.iter().position(|&c| c == column).ok_or_else(|| broken("a key column a leaf's batch lacks"))?;
        let waiting = std::mem::take(&mut self.waiting);
        let told: Vec<Told> = waiting.iter().map(|(_, t)| t.clone()).collect();
        self.judged.extend(waiting);
        if let Some(kept) = self.kept.take() {
            let kept = judged(kept, told, &self.layout.0, db)?;
            let filter = filter_of(&kept, (at, nulls), db)?;
            self.kept = Some(kept);
            return Ok(filter);
        }
        let all: Vec<Told> = self.judged.iter().map(|(_, t)| t.clone()).collect();
        let width = self.layout.1;
        let read = resolved(
            stream::batches_with(self.plan.clone(), (db, tx), &Deferred::all(width).with_told(all))?,
            crate::db::kernels::chosen(db),
        );
        let (kept, whole) = held(read, at, room)?;
        let filter = filter_of(&kept, (if whole { at } else { 0 }, nulls), db)?;
        if whole {
            self.kept = Some(kept);
        } else {
            self.past = true;
        }
        Ok(filter)
    }

    /// Its plan in the chain: its batches held, judged by the filters made
    /// for it since; else its plan, told every filter made for it, if any.
    ///
    /// # Errors
    ///
    /// As `filter`.
    fn finished(self, db: &Db) -> Result<Plan> {
        let Slot { plan, layout, kept, past: _, judged: mut all, waiting } = self;
        if let Some(kept) = kept {
            let told = waiting.into_iter().map(|(_, t)| t).collect();
            let kept = judged(kept, told, &layout.0, db)?;
            return Ok(Plan::Transfer(Box::new(Transfer::Held(HeldLeaf::held(layout, kept)))));
        }
        all.extend(waiting);
        if all.is_empty() {
            return Ok(plan);
        }
        let told = all.into_iter().map(|(_, t)| t).collect();
        Ok(Plan::Transfer(Box::new(Transfer::Held(HeldLeaf::told(layout, plan, told)))))
    }
}

/// `kept`, judged by `told`, a batch none of whose rows is kept left out.
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
fn judged(kept: Vec<Kept>, told: Vec<Told>, columns: &[usize], db: &Db) -> Result<Vec<Kept>> {
    if told.is_empty() {
        return Ok(kept);
    }
    let batches: Batches = Box::new(kept.into_iter().map(Ok));
    runtime::applied(batches, told, columns, (db, false))
        .filter(|k| k.as_ref().map_or(true, |(_, rows)| rows.as_ref().is_none_or(|r| !r.is_empty())))
        .collect()
}

/// The batches of `read`, held while their bytes fit `room`, which they
/// then take, and whether they did; past it, of the batches, the vector
/// at `at` alone, to make a filter of.
///
/// # Errors
///
/// `read`'s errors.
fn held(read: Batches, at: usize, room: &mut usize) -> Result<(Vec<Kept>, bool)> {
    let (mut kept, mut bytes, mut whole) = (Vec::new(), 0_usize, true);
    for batch in read {
        let (batch, rows) = batch?;
        if rows.as_deref().is_some_and(<[u32]>::is_empty) {
            continue;
        }
        bytes = bytes.saturating_add(batch.bytes()).saturating_add(rows.as_ref().map_or(0, |r| r.len().saturating_mul(size_of::<u32>())));
        if whole && bytes > *room {
            whole = false;
            kept = kept.into_iter().map(|(b, r)| (only(b, at), r)).collect();
        }
        kept.push(if whole { (batch, rows) } else { (only(batch, at), rows) });
    }
    if whole {
        *room = room.saturating_sub(bytes);
    }
    Ok((kept, whole))
}

/// `batch` of its vector at `at` alone.
fn only(batch: Batch, at: usize) -> Batch {
    let rows = batch.rows;
    Batch { columns: batch.columns.into_iter().nth(at).into_iter().collect(), rows }
}

/// \[DESIGN\] (RULES.md D369) The runtime filter of the keys of the vector
/// at `at` of the rows `kept` keeps, whose nulls match as `nulls` says,
/// hashed as a join's table hashes them; of no row, one that keeps none.
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
fn filter_of(kept: &[Kept], (at, nulls): (usize, bool), db: &State) -> Result<Option<Arc<RuntimeFilter>>> {
    let parts: Vec<(Batch, Vec<usize>)> = kept
        .iter()
        .filter_map(|(batch, rows)| {
            let column = batch.columns.get(at)?.clone();
            Some((Batch { columns: vec![column], rows: batch.rows }, places_of(rows.clone(), batch.rows)))
        })
        .collect();
    let keys = if parts.is_empty() { Batch::empty(1) } else { Batch::joined(parts) };
    let seed = db.functions.seed().clone();
    let how = Hashing { seed: &seed, keyed: false, direct: false, kernels: None };
    let mut hashes = KeyHashes::default();
    hashes.of_batch((&keys, None), (&[0], &[nulls]), how)?;
    Ok(RuntimeFilter::of_built(&keys, (0, nulls), &hashes, how).map(Arc::new))
}

// Unit tests: a phase's leaves held, judged and given back (RULES.md
// R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
