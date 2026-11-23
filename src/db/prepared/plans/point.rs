//! \[DESIGN\] A prepared query's plan kept, run as a point read where it
//! is one (RULES.md D752). Its probe is placed with the execution's
//! values and its row read without the executor's operators
//! (`exec/point.rs`).

use std::sync::Arc;

use crate::binder::BoundWrite;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::db::write::Written;
use crate::error::Result;
use crate::exec::Slots;
use crate::physical::{Kept, Plan};

/// How a prepared query's execution runs.
pub(in crate::db) enum Run {
    /// By its physical plan.
    Plan(Box<Plan>),
    /// By its plan kept: as its point read, if it is one, else opened as it
    /// runs, its correlated subqueries keeping what they settled (RULES.md
    /// D793).
    Point(Arc<Kept>),
}

/// How an execution with the values `slots` runs `plan`, a plan kept: by
/// it, as its point read with the values, if it is one, or else as
/// `Kept::opened` opens it as it runs (`Stream::run`), so what its
/// subqueries settled is kept with it (RULES.md D793).
pub(in crate::db) fn run(plan: &Arc<Kept>, slots: Option<Slots>) -> (Run, Option<Slots>) {
    (Run::Point(Arc::clone(plan)), slots)
}

/// `w`, a kept UPDATE or DELETE whose plan kept is `plan`, run with
/// `slots`. Its object row is found by the plan's point read, if it is
/// one, or else by the plan as `Kept::opened` opens it; by a plan made
/// with the values, if one of its probes' values raises (`Kept::places`,
/// RULES.md D793).
///
/// # Errors
///
/// As `Kept::opened` or `Written::slotted`.
pub(in crate::db) fn written(
    st: &State,
    w: Arc<BoundWrite>,
    (plan, reads): (&Arc<Kept>, Arc<[QualifiedName]>),
    slots: Slots,
) -> Result<Written> {
    if !plan.places(&slots) {
        return Written::slotted(w, None, slots);
    }
    if plan.point(st).is_some() {
        return Ok(Written::pointed(w, (Arc::clone(plan), reads), slots));
    }
    let (opened, _) = plan.opened(Some(Arc::clone(&slots)), crate::optimizer::prepared::budget(st))?;
    Written::slotted(w, Some((opened, reads)), slots)
}

impl super::Keeping<'_> {
    /// \[DESIGN\] Of a kept UPDATE or DELETE, its bound write, if it is
    /// kept with what holds now: what a compiled program's change looks at
    /// to learn whether it may run straight on its table (RULES.md D841).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn kept_write(&self) -> Result<Option<Arc<BoundWrite>>> {
        self.kept(|k| if let Some(super::KeptPlan::Write(w)) = &k.plan { Some(Arc::clone(w)) } else { None })
    }

    /// \[DESIGN\] A kept UPDATE or DELETE run with the values `slots`, as
    /// `write_bound` runs it once it is kept, past its binding: its object
    /// rows by its plan kept, learned or planned with the values as that
    /// plan stands (RULES.md D841); none if it is not kept.
    ///
    /// # Errors
    ///
    /// As `write_bound`.
    pub(in crate::db) fn written_kept(&self, slots: Slots) -> Result<Option<Written>> {
        let found =
            |kept: &super::Kept| if let Some(super::KeptPlan::Write(w)) = &kept.plan { Some((Arc::clone(w), kept.clocked)) } else { None };
        let Some((w, clocked)) = self.kept(found)? else { return Ok(None) };
        self.written(w, self.slots_of(slots, clocked)).map(Some)
    }
}
