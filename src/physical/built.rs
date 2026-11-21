//! \[DESIGN\] A plan as the planner builds it, with whether its rows come
//! out in the order asked for.

use super::Plan;
use crate::logical::{Cond, SortKey};

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

/// `want`, if each of its keys is of a product's left operand, `width`
/// columns wide, whose order the product keeps; else none.
pub(super) fn of_left(want: &[SortKey], width: usize) -> &[SortKey] {
    if want.iter().all(|k| k.column < width) { want } else { &[] }
}

/// An outer join's own conditions, of its left operand's columns and of
/// its right's (`Plan::OuterJoin`), if either has one.
pub type Own = Option<Box<(Option<Cond>, Option<Cond>)>>;
