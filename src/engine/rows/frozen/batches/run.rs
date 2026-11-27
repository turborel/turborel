//! \[DESIGN\] (RULES.md D360, D361) A frozen run of a column store's scan
//! made a batch. The conjuncts the scan judges are judged first, of their
//! own columns (`columns/judged.rs`), of the rows the tree has no entry
//! for; if they keep one row in eight or more, the run is decoded whole,
//! the columns decoded to judge it given as they are, and the filter
//! selects of it; else the places of the rows they
//! keep are held, and a batch is decoded of them at those places alone,
//! once they make half a batch or the group ends, so a selective filter
//! gives full batches, not a few rows each, which every operator above
//! would pay a batch's overhead for. A column asked for late is given as
//! its rows' places (`storage::Late`), decoded once an operator reads it,
//! of the rows still kept then.
//!
//! \[DESIGN\] (RULES.md D1266) With each batch of frozen rows the scan
//! notes what it judged of them (`Batched::vetted`): of a run given
//! whole, if a conjunct was judged exactly of it, the rows its conjuncts
//! kept, which the filter starts from, the conjuncts judged exactly of
//! those rows, and the one judged of the first of them alone, if one
//! was; of a batch of the rows kept of
//! several runs, the conjuncts every one of those runs judged exactly. The filter over the scan does not judge those again
//! (`exec/batch/vetted.rs`).

use std::sync::Arc;

use super::Batched;
use super::pending::Pending;
use crate::engine::columns::group::Group;
use crate::engine::columns::judged::KEPT_SHARE;
use crate::engine::columns::scan::{column_at, column_of_run};
use crate::error::Result;
use crate::storage::{BATCH_ROWS, Batch, Column, Late, Vetted};
use crate::value::Value;

/// \[DESIGN\] The rows kept of a group's runs a batch is decoded of once
/// held: half a batch, so a batch made of kept rows is at least half as
/// large as one of a whole run.
const KEPT_ROWS: usize = BATCH_ROWS >> 1_u32;

impl Batched {
    /// The batch of the run `from` to `to` of the group being read, if one
    /// is ready: of the rows the tree has no entry for, those the scan's
    /// conjuncts keep, if it judges any.
    pub(super) fn run_batch(&mut self, (from, to): (u32, u32)) -> Result<Option<Batch>> {
        let Some(group) = self.group.as_ref().map(|(group, _, _)| Arc::clone(group)) else { return Ok(None) };
        let positions = self.unshadowed((from, to));
        if self.judgment.is_empty() {
            return match positions {
                None => self.dense(&group, (from, to), (Vec::new(), true)).map(Some),
                Some(p) => (!p.is_empty()).then(|| self.at_positions(&group, &p, Vec::new())).transpose(),
            };
        }
        let shadowed = positions.is_some();
        let given = positions.map(|p| p.iter().map(|&q| q.saturating_sub(from)).collect());
        let pages = self.table.pages_for(&group)?;
        let types = self.table.format.types();
        let judged = self.judgment.judge(&group, (&pages, &mut self.prepared), (types, (from, to)), given)?;
        let rows = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
        let kept: Vec<u32> = judged.kept.iter().map(|&r| from.saturating_add(r)).collect();
        let (exact, partly) = (judged.exact, judged.partly);
        // Of many rows kept, the run as it is, which the filter selects of;
        // but a short run, as imprints leave (RULES.md D511), is gathered
        // with others, so batches stay full.
        let whole = rows >= KEPT_ROWS || !group.imprinted();
        if self.pending.is_empty() && whole && kept.len().saturating_mul(KEPT_SHARE) >= rows {
            if !shadowed {
                // Under a sort's bound the sort keeps few of the rows
                // given, so late columns stay late (RULES.md D1241).
                let batch = self.dense(&group, (from, to), (judged.decoded, self.judgment.bounded()))?;
                // The run whole, with the rows of it judging kept, whose
                // places from its first are the batch's (RULES.md D1266):
                // said only if a conjunct was judged exactly, as else the
                // filter passes over nothing, and it judges a batch whole
                // more cheaply than most of its rows picked out.
                self.vetted = (exact != 0).then(|| Vetted { rows: (judged.kept.len() < rows).then_some(judged.kept), exact, partly });
                return Ok(Some(batch));
            }
            let mut once = Pending::default();
            once.add(&kept, (&judged.kept, &judged.decoded), exact);
            return self.held_batch(&group, once.take()).map(Some);
        }
        let these = (judged.kept.as_slice(), judged.decoded.as_slice());
        if !self.pending.is_empty() && self.pending.len().saturating_add(kept.len()) > BATCH_ROWS {
            let ready = self.pending.take();
            self.pending.add(&kept, these, exact);
            return self.held_batch(&group, ready).map(Some);
        }
        self.pending.add(&kept, these, exact);
        if self.pending.len() < KEPT_ROWS {
            return Ok(None);
        }
        let ready = self.pending.take();
        self.held_batch(&group, ready).map(Some)
    }

    /// The batch of the rows kept of the group's runs and not yet given,
    /// if any is: as its last run is read.
    pub(super) fn kept_batch(&mut self) -> Result<Option<Batch>> {
        let Some(group) = self.group.as_ref().map(|(group, _, _)| Arc::clone(group)).filter(|_| !self.pending.is_empty()) else {
            return Ok(None);
        };
        let ready = self.pending.take();
        self.held_batch(&group, ready).map(Some)
    }

    /// The batch of every row of the run `from` to `to` of `group`, each
    /// column read a chunk at a time, those in `decoded` as they are, late
    /// ones not decoded if `late`: not of a run the scan judged and kept
    /// many rows of, as those are decoded as cheaply whole as late, but
    /// under a sort's bound, whose sort keeps few of the rows it is given.
    fn dense(&mut self, group: &Group, (from, to): (u32, u32), (mut decoded, late): (Vec<(usize, Column)>, bool)) -> Result<Batch> {
        let rows = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
        let pages = self.table.pages_for(group)?;
        let types = self.table.format.types();
        let mut places: Option<Arc<[u32]>> = None;
        let mut batch = Batch { columns: Vec::with_capacity(self.columns.len()), rows };
        for &c in &self.columns {
            if let Some(column) = late.then(|| self.late_column(c, &mut places, || (from..to).collect())).flatten() {
                batch.columns.push(column);
                continue;
            }
            if let Some(at) = decoded.iter().position(|(d, _)| *d == c) {
                batch.columns.push(decoded.swap_remove(at).1);
                continue;
            }
            batch.columns.push(match types.get(c) {
                // A run whole is read a chunk at a time, each value once.
                Some(t) => column_of_run(group, (&pages, &mut self.prepared), (c, t), (from, to))?,
                None => Column::from(vec![Value::Null; rows]),
            });
        }
        Ok(batch)
    }

    /// The batch of the rows held at `positions` of `group`, the columns
    /// of `held` as they are, each other decoded at them alone (RULES.md
    /// D366), every row of it one judging kept, `exact` the conjuncts
    /// judged exactly of them all (RULES.md D1266).
    fn held_batch(&mut self, group: &Group, (positions, held, exact): (Vec<u32>, Vec<(usize, Column)>, u64)) -> Result<Batch> {
        let batch = self.at_positions(group, &positions, held)?;
        self.vetted = Some(Vetted { rows: None, exact, partly: None });
        Ok(batch)
    }

    /// The batch of the rows at `positions` of `group`, ascending, those
    /// of `held` as they are, each other column decoded at them alone,
    /// late ones not decoded.
    fn at_positions(&mut self, group: &Group, positions: &[u32], mut held: Vec<(usize, Column)>) -> Result<Batch> {
        let pages = self.table.pages_for(group)?;
        let types = self.table.format.types();
        let mut places: Option<Arc<[u32]>> = None;
        let mut batch = Batch { columns: Vec::with_capacity(self.columns.len()), rows: positions.len() };
        for &c in &self.columns {
            if let Some(late) = self.late_column(c, &mut places, || positions.to_vec()) {
                batch.columns.push(late);
                continue;
            }
            if let Some(at) = held.iter().position(|(h, _)| *h == c) {
                batch.columns.push(held.swap_remove(at).1);
                continue;
            }
            batch.columns.push(match types.get(c) {
                Some(t) => column_at(group, (&pages, &mut self.prepared), (c, t), positions)?,
                None => Column::from(vec![Value::Null; positions.len()]),
            });
        }
        Ok(batch)
    }

    /// Column `c` given late, if it is asked for so: of the rows at
    /// `places`, made by `made` once for every late column of a batch.
    fn late_column(&self, c: usize, places: &mut Option<Arc<[u32]>>, made: impl FnOnce() -> Vec<u32>) -> Option<Column> {
        let source = self.source.as_ref().filter(|_| self.late.contains(&c))?;
        let at = Arc::clone(places.get_or_insert_with(|| made().into()));
        Some(Column::Late(Late::new(Arc::clone(source), c, at)))
    }
}
