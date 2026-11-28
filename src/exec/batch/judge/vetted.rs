//! \[DESIGN\] (RULES.md D1266) The first filter over a scan told its
//! conjuncts. The scan judges those conjuncts of each frozen run before it
//! decodes the rest of the run's columns, and says with each batch which
//! rows it kept and which conjuncts it judged exactly of them
//! (`storage::Vetted`). This filter judges only the rows kept, as the
//! rows a filter before kept (`Judging::kept_of`), and passes over each
//! of its conjuncts whose own told conjunct the scan says it judged
//! exactly, so no conjunct is evaluated twice of a row.
//!
//! A conjunct passed over is one that cannot raise an error and gives no
//! warning: a scan is told nothing of a condition that can raise
//! (`logical::pushed_conjuncts`), and the conjuncts told, a comparison of
//! a column with a constant, IS NULL, IN and NOT IN constants, an OR of
//! those and a LIKE of a constant pattern, complete no condition of a
//! warning. So nothing a row would have reported is lost with its second
//! judging. A filter that can raise an error hears nothing and is given
//! every row of each batch, as it was before the scan said anything
//! (`stream.rs`, `parallel/pipeline.rs`).
//!
//! No other filter hears a scan. One over a projection, a join or a
//! second filter judges rows or columns that are no longer the scan's. A
//! filter whose subqueries are made constants judges by the filter made
//! of that other condition (`judge/bound.rs`), whose conjuncts' places are
//! not those the scan was told: it is made as any filter is, hears
//! nothing, and judges every conjunct of the rows the scan kept.

use std::sync::Arc;

use super::super::vetted::Heard;
use super::{Judged, Judging, Said, placed};
use crate::exec::Runner;
use crate::logical::Cond;
use crate::storage::Batch;
use crate::storage::sel::rows_of;

impl Judging {
    /// As `Judging::of`, of the filter of `cond` directly over a scan of
    /// `columns` whose condition begins with the conjuncts `cond` tells
    /// it (`logical::pushed_conjuncts`): each conjunct judged over vectors
    /// knows the told conjunct that is it.
    pub(in crate::exec) fn over_scan(cond: &Cond, columns: &[usize]) -> Judging {
        let mut judging = Judging::of(cond, columns);
        (judging.judged, judging.told) = placed(cond, columns).0.into_iter().unzip();
        judging
    }

    /// What its scan says of the batch it is handed now: nothing, of a
    /// filter that hears no scan.
    pub(super) fn heard(&self) -> Heard {
        self.said.as_ref().map(|said| said.heard()).unwrap_or_default()
    }

    /// The conjunct `heard` says its scan judged of the batch's first
    /// rows alone, and the place of the first row the scan did not judge
    /// it of: none if it names none.
    pub(super) fn partly_judged(&self, heard: Heard) -> Option<(&Judged, u32)> {
        let (bit, from) = heard.partly;
        let n = self.told.iter().position(|told| told & bit != 0)?;
        Some((self.judged.get(n)?, from))
    }

    /// It, judging the stream of one scan, which says to `said` what it
    /// judged of each batch: heard only by a filter made `over_scan` one
    /// of whose conjuncts the scan was told as itself, so no other filter
    /// of a pipeline's steps passes over a conjunct.
    pub(in crate::exec) fn hearing(&self, said: Option<&Arc<Said>>) -> Judging {
        let mut judging = self.clone();
        judging.said = said.filter(|_| self.told.iter().any(|told| *told != 0)).cloned();
        judging
    }
}

impl Judged {
    /// \[DESIGN\] (RULES.md D1266) `kept`, the rows of `batch`, of those
    /// at `rows` or of all, a filter's other conjuncts keep, less those
    /// this conjunct is false of, of a batch whose scan judged it of the
    /// rows before the place `from` alone: the rows kept before that
    /// place the scan found it true of, and they stay; those from that
    /// place on it kept unjudged, and the conjunct is judged of them
    /// here. None if it gives none over the batch's vectors, the batch
    /// then judged whole a row at a time.
    pub(super) fn rest_kept(
        &self,
        from: u32,
        kept: Option<Vec<u32>>,
        (batch, rows): (&Batch, Option<&[u32]>),
        runner: &Runner,
    ) -> Option<Vec<u32>> {
        let mut settled = kept.unwrap_or_else(|| rows.map_or_else(|| rows_of(0..batch.rows), <[u32]>::to_vec));
        let rest = settled.split_off(settled.partition_point(|&r| r < from));
        settled.extend(self.kept(batch, Some(&rest), runner, runner.kernels())?);
        Some(settled)
    }
}

// Unit tests: which conjunct knows which told place, and what a filter
// judges of a batch its scan vouches for (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
