//! \[DESIGN\] A range of an index whose first key columns its probe holds
//! equal, of a primary or unique key or of an index holding every column
//! read, is never priced above the scan of the rows its conditions may
//! match (RULES.md D839).
//!
//! Every row such a range finds matches the conjuncts it answers, so a
//! scan told those conjuncts reads it too, in the zones it may lie in
//! (`TableStorage::scan_share`), and the range reads none past them. Its
//! cost by its estimated rows, each fetched by row id, can still pass the
//! scan's, as where its estimate is high by some times, from statistics
//! gathered before the rows of its range were written, or where the scan's
//! share of a sample of zones is low. A plan kept with such a scan reads
//! the zones of every later execution's values, though the range would
//! read their rows alone. So the range's cost is at most that scan's.
//! Where the two are then priced alike, the range is taken if it is
//! estimated to read fewer rows than the scan, and fewer than the table's
//! rows over PLANNER.ESTIMATE_MARGIN, as the joins weigh their estimates
//! (D386). Where it reads a share of the scan's rows, it fetches no more
//! than that share. Where it reads most of them, the zones the scan reads
//! hold little else, so its rows lie together, in the order of their row
//! ids, and each fetch starts at the leaf of the last (`engine/tree/near.rs`).
//! A range of most
//! of the table, whose zones may not tell, is left to the costs.

use super::{Candidate, On, Path, keyed};
use crate::catalog::IndexDesc;
use crate::logical::Cond;
use crate::physical::PhysicalPlanner;

/// Whether `c`, a candidate of `ix`, `covering` if its key holds every
/// column read, is a range bounded by the scan of its conjuncts, a probe of
/// keys holding a prefix of them equal, each conjunct it uses answered
/// exactly, of a primary or unique key or a covering index.
pub(super) fn bounded(ix: &IndexDesc, c: &Candidate, covering: bool) -> bool {
    c.fixed > 0 && !c.loose && (ix.unique || covering) && c.probes.iter().all(keyed)
}

/// Whether `path`, an index range a scan bounds, priced as `best` is, is
/// taken at the tie, where it is estimated to read fewer rows than `best`,
/// and fewer than the table's `all` over `margin` (PLANNER.ESTIMATE_MARGIN),
/// so not most of the table even if its estimate is that many times low.
pub(super) fn fewer_alike(path: &Path, best: &Path, (margin, all): (f64, f64)) -> bool {
    path.bounded && path.total <= best.total && path.est < best.est && path.est * margin < all
}

impl PhysicalPlanner<'_> {
    /// `cost`, the cost of reading `on`'s table by the candidate `c`, at
    /// most what a scan told the conjuncts `c` answers costs, those of
    /// `on.conds` it uses, each `?` filled as the scan's are
    /// (`PhysicalPlanner::priced`). The scan is priced only if `cost` is
    /// above the scan of every conjunct, `scan`, as a scan told fewer of
    /// them reads no fewer zones.
    pub(super) fn bounded_cost(&self, (on, c): (&On<'_>, &Candidate), needed: &[usize], cost: f64) -> f64 {
        if cost <= on.scan {
            return cost;
        }
        let used: Vec<Cond> = c.used.iter().filter_map(|&at| on.conds.get(at).cloned()).collect();
        let (scan, rows) = super::scan_priced(on.storage, needed, &self.priced(&used));
        cost.min(scan + rows * on.evaluation.per_row)
    }
}

// Unit tests: which candidates a scan bounds, and the cost it bounds
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
