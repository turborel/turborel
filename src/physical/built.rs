//! \[DESIGN\] A plan as the planner builds it, with whether its rows come
//! out in the order asked for.

use super::Plan;

/// A built plan, and whether its rows come out in the order that was asked for.
pub(super) struct Built {
    /// The plan.
    pub(super) plan: Plan,
    /// Whether its rows come out in the order asked for.
    pub(super) ordered: bool,
}

impl Built {
    /// `plan`, its rows in no order asked for.
    pub(super) fn unordered(plan: Plan) -> Built {
        Built { plan, ordered: false }
    }
}
