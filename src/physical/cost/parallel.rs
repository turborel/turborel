//! \[DESIGN\] (RULES.md D652) The degree a part will run at, as costs see
//! it before the marking pass (`physical/parallel.rs`, RULES.md D291)
//! marks it, by the same rule: as many threads as morsels of
//! EXECUTOR.MORSEL_ROWS the part's tables hold, up to EXECUTOR.DEGREE or
//! the workers, and one for a part reading fewer than
//! EXECUTOR.PARALLEL_MIN_ROWS. A join's method is chosen seeing it: a
//! hash join's scan of its built operand and the hashing of either
//! operand's rows are shared by its part's threads, where an index
//! nested-loop join's probes run on the statement's thread, so an index
//! join is no longer taken over a hash join that eight threads would run
//! (finding N7 of the planner's review). A part's threads cost
//! `PART_START` to begin, as `bench/analytics/costs.rs` measures it, so a
//! part only just past the least rows gains less than its degree.

use crate::db::State;
use crate::settings::{EXECUTOR_DEGREE, EXECUTOR_MORSEL_ROWS, EXECUTOR_PARALLEL_MIN_ROWS};
use crate::storage::cost_of;

/// \[DESIGN\] What beginning a part on several threads costs, in rows of
/// a scan: its workers woken, its morsels handed out and its pieces
/// merged (`bench/analytics/costs.rs`, its part of two sizes).
pub(in crate::physical) const PART_START: f64 = 2_000.0;

/// What the settings ask of a statement's parts: the most threads one
/// runs on, the rows of a morsel, and the least rows one reads in
/// parallel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Degrees {
    /// The most threads a part runs on, the statement's own among them.
    pub most: usize,
    /// The rows of a morsel.
    pub morsel: usize,
    /// The least rows a part reads in parallel.
    pub least: usize,
}

impl Default for Degrees {
    /// One thread: no part runs in parallel.
    fn default() -> Self {
        Degrees { most: 1, morsel: 1, least: 0 }
    }
}

impl Degrees {
    /// What `db`'s settings ask.
    pub(crate) fn of(db: &State) -> Degrees {
        let workers = crate::physical::parallel::workers_of(db);
        let most = match db.settings.count(EXECUTOR_DEGREE) {
            0 => workers,
            n => n.min(workers.max(1)),
        };
        Degrees { most, morsel: db.settings.count(EXECUTOR_MORSEL_ROWS).max(1), least: db.settings.count(EXECUTOR_PARALLEL_MIN_ROWS) }
    }

    /// The threads a part whose tables hold `rows` rows runs on.
    pub(crate) fn degree(self, rows: usize) -> usize {
        if rows < self.least {
            return 1;
        }
        self.most.min(rows.div_ceil(self.morsel.max(1))).max(1)
    }

    /// `degree` of rows as a cost.
    pub(in crate::physical) fn degree_of(self, rows: f64) -> f64 {
        cost_of(self.degree(crate::physical::parallel::rows_of(rows)))
    }
}
