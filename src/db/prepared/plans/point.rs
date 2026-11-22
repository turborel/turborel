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
    /// As the point read of its plan kept.
    Point(Arc<Kept>),
}

/// How an execution with the values `slots` runs `plan`, a plan kept. It
/// runs as its point read with the values, if it is one, or else as
/// `Kept::opened` opens it, with the values it reads where its slots are.
///
/// # Errors
///
/// As `Kept::opened`.
pub(in crate::db) fn run(st: &State, plan: &Arc<Kept>, slots: Option<Slots>) -> Result<(Run, Option<Slots>)> {
    if plan.point(st).is_some() {
        return Ok((Run::Point(Arc::clone(plan)), slots));
    }
    let (opened, slots) = plan.opened(slots, crate::optimizer::prepared::budget(st))?;
    Ok((Run::Plan(Box::new(opened)), slots))
}

/// `w`, a kept UPDATE or DELETE whose plan kept is `plan`, run with
/// `slots`. Its object row is found by the plan's point read, if it is
/// one, or else by the plan as `Kept::opened` opens it.
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
    if plan.point(st).is_some() {
        return Ok(Written::pointed(w, (Arc::clone(plan), reads), slots));
    }
    let (opened, _) = plan.opened(Some(Arc::clone(&slots)), crate::optimizer::prepared::budget(st))?;
    Written::slotted(w, Some((opened, reads)), slots)
}
