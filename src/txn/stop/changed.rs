//! \[EXTENSION\] The plan of the rows a data change statement changes,
//! counted, when EXPLAIN ANALYZE runs the statement (RULES.md D622). The
//! session's `Stop` holds it, as the statement reaches the `Stop` through
//! its transaction wherever it plans: the first plan the statement makes
//! of the rows it reads takes the want and is counted
//! (`db/explain/changed.rs`), so a plan a trigger it fires makes after is
//! not.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::physical::{NodeCount, Plan};
use crate::sync::lock_builtin;

/// \[DESIGN\] A plan counted, and each operator's count.
pub(crate) type CountedPlan = (Plan, Vec<Arc<NodeCount>>);

/// The plan of a data change EXPLAIN ANALYZE runs.
#[derive(Debug, Default)]
pub(crate) struct Changed {
    /// Whether the next plan of rows to change is to be counted.
    wanted: AtomicBool,
    /// That plan, counted, once made.
    kept: Mutex<Option<CountedPlan>>,
}

impl Changed {
    /// Sets whether the next plan of rows to change is to be counted.
    pub(crate) fn want(&self, wanted: bool) {
        self.wanted.store(wanted, Ordering::Relaxed);
    }

    /// Whether the plan made now is the one to count: true once a want.
    pub(crate) fn taken(&self) -> bool {
        self.wanted.load(Ordering::Relaxed) && self.wanted.swap(false, Ordering::Relaxed)
    }

    /// Keeps `plan`, the plan counted.
    pub(crate) fn keep(&self, plan: CountedPlan) {
        *lock_builtin(&self.kept) = Some(plan);
    }

    /// The plan kept, taken.
    pub(crate) fn kept(&self) -> Option<CountedPlan> {
        lock_builtin(&self.kept).take()
    }
}

// Unit tests: a want taken once (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D622: no plan is taken unless one is
    /// wanted, a want is taken once, and a plan kept is taken once.
    #[test]
    fn a_want_is_taken_once() {
        let c = Changed::default();
        assert!(!c.taken());
        c.want(true);
        assert!(c.taken() && !c.taken());
        c.want(true);
        c.want(false);
        assert!(!c.taken());
        c.keep((Plan::Values { rows: Vec::new(), detail: String::new() }, Vec::new()));
        assert!(c.kept().is_some() && c.kept().is_none());
    }
}
