//! \[DESIGN\] (RULES.md D1096) A late column of a hash join's built
//! operand. A join that keeps a left row gives its built row's columns
//! that no operator above reads before the stream is read whole as the
//! built rows of its rows, not their values, so a join above gathers four
//! bytes a row of them, never a text, and they are gathered once, of the
//! rows every join kept, as the stream is read (Abadi, Myers, DeWitt and
//! Madden, ICDE 2007, on late materialization).
//!
//! Its rows are of its own batch alone: a reader that did not resolve it
//! gathers those rows (`Late::made`), never the operand whole, and no
//! batch's gathering widens another's. It holds the operand while it
//! lives, which the join's grant is charged for: every reader of a join's
//! stream gathers it as each batch is given, while the join lives
//! (`exec/batch/late.rs`, `resolved`), and an operator that keeps batches
//! gathers it as it takes them (`Batch::resolve_built`).

use std::sync::{Arc, OnceLock};

use super::super::mask::index;
use super::super::{Batch, Column, NO_ROW, Picks};
use super::{Late, Places};
use crate::kernels::Kernels;

/// The rows of a late column of a built operand.
#[derive(Clone)]
pub(super) struct Built {
    right: Arc<Batch>,
    /// Of each row, its row of `right`, or `NO_ROW` for a null.
    rows: Arc<[u32]>,
}

impl Late {
    /// \[DESIGN\] (RULES.md D1096) Vector `column` of `right`, a join's
    /// built operand, of its rows `rows`, `NO_ROW` for a null; none if it
    /// has no such vector or that vector is late itself.
    pub fn built(right: &Arc<Batch>, column: usize, rows: &Arc<[u32]>) -> Option<Late> {
        if !matches!(right.columns.get(column), Some(c) if !matches!(c, Column::Late(_))) {
            return None;
        }
        let places = Places::Built(Built { right: Arc::clone(right), rows: Arc::clone(rows) });
        Some(Late { places, column, made: OnceLock::new() })
    }

    /// Whether it is of a built operand.
    pub fn is_built(&self) -> bool {
        matches!(self.places, Places::Built(_))
    }
}

impl Built {
    /// How many rows.
    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Vector `column` of its operand at its rows `rows`, or all, the
    /// others null: one gather of those rows alone.
    pub(super) fn resolved(&self, column: usize, rows: Option<&[u32]>, kernels: &dyn Kernels) -> Column {
        let Some(vector) = self.right.columns.get(column) else { return Column::nulls(self.len()) };
        let Some(rows) = rows else { return vector.gathered_by(&Picks::new(&self.rows), kernels) };
        let mut picks = vec![NO_ROW; self.len()];
        for &r in rows {
            if let (Some(slot), Some(&built)) = (picks.get_mut(index(r)), self.rows.get(index(r))) {
                *slot = built;
            }
        }
        vector.gathered_by(&Picks::new(&picks), kernels)
    }

    /// The rows at `picks`, in order, a null for none.
    pub(super) fn gathered(&self, picks: &[Option<usize>]) -> Built {
        let rows: Vec<u32> = picks.iter().map(|p| p.and_then(|r| self.rows.get(r).copied()).unwrap_or(NO_ROW)).collect();
        Built { right: Arc::clone(&self.right), rows: rows.into() }
    }
}
