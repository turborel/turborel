//! A window's columns pruned (`blind/pruned.rs`, RULES.md D1167, D1245).
//! A window reads its partitioning's columns, its ordering's and those
//! of its calls' arguments, filters and frames' offsets, and passes on
//! its input's columns read above it. Every call is still computed,
//! whether or not one above reads it, so no error it raises is lost. Its
//! input is pruned to those columns as a sort's is, so the scan under it
//! reads them alone. A window that searches for a row pattern, or whose
//! call holds a subquery, is left as it is.

use super::super::super::{PhysicalPlanner, Plan};
use super::super::width::Sized;
use super::Live;
use super::single::plain;
use crate::logical::{Window, expr_columns};

impl PhysicalPlanner<'_> {
    /// Prunes `plan`, a window, its input of the widths `sizes`: its
    /// input to the columns it reads and those above read of it, or, if
    /// it is not plain, whole. A function of its own, so the walk's frame
    /// holds none of its locals.
    #[inline(never)]
    pub(super) fn window_pruned(&self, plan: &mut Plan, sizes: &Sized, needed: &[usize]) -> Live {
        let (Some(width), true) = (sizes.width, plain_window(plan)) else { return self.read_whole(plan) };
        let Plan::Window { window, .. } = plan else { return None };
        let reads = window_reads(window, width, needed);
        let live = self.operand_pruned(&mut window.input, sizes, &reads)?;
        Some(live.into_iter().chain(width..width.saturating_add(window.calls.len())).collect())
    }
}

/// Whether `plan` is a window that reads nothing but the columns it
/// names: no row pattern searched, no subquery in a call.
fn plain_window(plan: &Plan) -> bool {
    let Plan::Window { window, .. } = plan else { return false };
    window.calls.iter().all(|c| c.frame.matching.is_none()) && window.exprs().all(plain)
}

/// The columns of its input of `width` that `window` reads, and those of
/// them read above it, of its columns `needed`: its calls' columns are
/// read of none. A call's nested window function is read in its argument
/// as a column past its input's (`logical/window.rs`), not one of them.
pub(super) fn window_reads(window: &Window<Plan>, width: usize, needed: &[usize]) -> Vec<usize> {
    let mut reads: Vec<usize> = needed.iter().copied().filter(|&c| c < width).collect();
    reads.extend(window.partition.iter().copied());
    reads.extend(window.order.iter().map(|k| k.column));
    let mut of_calls = Vec::new();
    window.exprs().for_each(|e| expr_columns(e, &mut of_calls));
    reads.extend(of_calls.into_iter().filter(|&c| c < width));
    reads
}
