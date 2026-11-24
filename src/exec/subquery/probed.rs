//! \[DESIGN\] RULES.md D790, D791: a correlated subquery whose run is one probe
//! of a whole unique key stays probed while its probes cost less than its
//! decorrelated build.
//!
//! A decorrelated subquery is run once without its equalities with outer
//! references, and each of its rows built into a keyed table, so the
//! build costs as many rows as the subquery gives without them: of a
//! scalar subquery over a table's key, the whole table. A run whose plan
//! is kept as one probe of a whole unique key (`planned.rs`) costs one
//! descent. So such a subquery is decorrelated only once the probes it
//! made, each weighed as `PROBE_ROWS` rows built, reach the rows its build
//! would hold, as estimated: past that, the runs to come are as likely as
//! those made to repay a build, and the probes made cost at most what the
//! build does. Its runs before then are probes; a subquery whose run is
//! not such a probe is decorrelated after EXECUTOR.DECORRELATE_AFTER_RUNS
//! runs, as before.

use std::sync::Arc;

use super::Runner;
use super::planned::{KeptRun, Planned};
use crate::logical::{Decorrelated, Subquery};
use crate::physical::estimated_rows;

/// \[DESIGN\] The rows built into a keyed table that cost as much as one
/// probe of a unique key: a descent of the key's index and a fetch of its
/// row, against a scan's row hashed and inserted (RULES.md D791). Measured
/// by `bench/joins/probed.rs` over inner tables of 1,000, 10,000 and
/// 100,000 rows, the ratio of the build's rows a second to the probes a
/// second was 15.6, 18.8 and 11.3; this is their geometric mean, 14.9,
/// rounded.
pub(super) const PROBE_ROWS: f64 = 15.0;

impl Runner {
    /// How `subquery`, at `at`, is decorrelated, if it can be. Of one
    /// whose plan is kept that is found once and kept with the plan
    /// (RULES.md D900), as it follows from the subquery alone, where each
    /// run past EXECUTOR.DECORRELATE_AFTER_RUNS that stayed probed made
    /// it again.
    pub(super) fn decorrelation_of(&self, subquery: &Subquery, at: u64) -> Option<Arc<Decorrelated>> {
        let kept = self.seen.borrow().get(&at).and_then(|seen| kept_of(&seen.planned));
        match kept {
            Some(kept) => kept.decorrelated(subquery),
            None => subquery.decorrelated().map(Arc::new),
        }
    }

    /// Whether the subquery at `at`, run `runs` times so far, decorrelated
    /// as `how` says, is still cheaper probed: its run's plan is kept as a
    /// probe of a whole unique key, and its probes, weighed as `PROBE_ROWS`
    /// rows each, come to fewer than the rows its build would hold.
    pub(super) fn probes_pay(&self, at: u64, runs: usize, how: &Decorrelated) -> bool {
        let (kept, known) = match self.seen.borrow().get(&at) {
            Some(seen) => (kept_of(&seen.planned), seen.built_rows),
            None => (None, None),
        };
        // A plan learned over its runs is kept too, and may read many
        // rows a run (RULES.md D1046).
        if !kept.is_some_and(|kept| kept.probes(&self.db)) {
            return false;
        }
        let built = known.unwrap_or_else(|| estimated_rows(&self.db, &how.plan));
        if let Some(seen) = self.seen.borrow_mut().get_mut(&at) {
            seen.built_rows = Some(built);
        }
        crate::storage::cost_of(runs) * PROBE_ROWS < built
    }
}

/// The plan `planned` keeps, if it keeps one.
fn kept_of(planned: &Planned) -> Option<Arc<KeptRun>> {
    match planned {
        Planned::Kept(kept) => Some(Arc::clone(kept)),
        Planned::Untried | Planned::Learning(..) | Planned::Varies => None,
    }
}

#[cfg(test)]
mod tests;
