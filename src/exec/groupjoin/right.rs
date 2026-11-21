//! \[DESIGN\] (RULES.md D490, D38) The bytes the join a groupjoin stands
//! for would hold of its right operand, counted as the join counts them
//! (`batch/joined/held.rs`). Past its work area that join is partitioned,
//! and a join whose left order is not asked for gives its rows partition
//! by partition (`join/partitioned.rs`), so the grouping over it gives
//! its groups in an order no groupjoin gives. The groupjoin then gives
//! way to that grouping, whose rows it must give.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::exec::batch::TABLE_ROW_BYTES;
use crate::storage::Batch;

/// The bytes of a join's right operand read so far, and the most it holds
/// before it is partitioned; shared by the threads that read its morsels.
pub(in crate::exec) struct RightBytes {
    bytes: AtomicUsize,
    limit: usize,
}

impl RightBytes {
    /// No bytes read yet, of a join that holds at most `limit`.
    pub(in crate::exec) fn new(limit: usize) -> RightBytes {
        RightBytes { bytes: AtomicUsize::new(0), limit }
    }

    /// Counts `batch`, of which `rows` are given, as the join counts the
    /// batches it holds; whether the join still holds them all.
    pub(in crate::exec) fn read(&self, batch: &Batch, rows: usize) -> bool {
        let more = batch.bytes().saturating_add(rows.saturating_mul(TABLE_ROW_BYTES));
        let before = self.bytes.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |b| Some(b.saturating_add(more))).unwrap_or_else(|b| b);
        before.saturating_add(more) <= self.limit
    }

    /// Whether the join would have outgrown its work area.
    pub(in crate::exec) fn past(&self) -> bool {
        self.bytes.load(Ordering::Relaxed) > self.limit
    }
}
