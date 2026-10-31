//! \[DESIGN\] Conditions moved into the operands of outer joins [ISO/IEC
//! 9075:1992 §7.5 GR1c, GR2-GR5]: a WHERE conjunct into the operand it
//! preserves, an ON conjunct into the operand that supplies nulls; and an
//! outer join whose null rows the WHERE clause rejects made the join it is
//! then. Split from optimizer.rs (RULES.md R-SIZE-2).

use super::raising::guarded;
use super::{Rule, RuleContext, wrap};
use crate::exec::{NoSubqueries, eval_cond};
use crate::logical::{Cond, LogicalPlan, OuterJoin, Truth, cond_columns, map_cond_columns, plan_has_outer, split_and};
use crate::value::Value;

/// See [`super::CONSTANT_FOLDING`].
pub const PUSH_FILTER_INTO_OUTER_JOIN: &str = "push_filter_into_outer_join";

/// Moves conditions into the operands of an outer join, where they keep the
/// same rows ([ISO/IEC 9075:1992 §7.5 GR5]: a preserved operand's rows that
/// match none are kept, with nulls):
///
/// * A WHERE conjunct over the preserved operand's columns alone, of a LEFT
///   or RIGHT join, keeps the rows of the join made from rows of that
///   operand it keeps: the same, filtered first.
/// * A WHERE conjunct over the other operand's columns alone that is not
///   true when they are null keeps none of the rows a preserved operand
///   gives with nulls: the join is the inner join, or, of a FULL join, the
///   outer join preserving the other operand. That conjunct is evaluated
///   here once, on nulls.
/// * An ON conjunct of a LEFT or RIGHT join over the columns of the operand
///   that supplies nulls alone: a row of that operand it is not true of
///   matches no row, as if it were not there.
///
/// Only conditions that cannot raise an error move, as they are then
/// evaluated on other rows (RULES.md D15); but an ON conjunct that can, of
/// the columns of the operand that supplies nulls alone, moves onto that
/// operand over a guard, an EXISTS of the other (`optimizer/raising.rs`),
/// if every ON conjunct that can raise is such. The ON condition is
/// applied to every row of the product of the operands [§7.5 GR1c], so
/// such a conjunct is evaluated on every row of its operand when the other
/// has a row, and on none when it has none, as over the guard. One of the
/// preserved operand's columns stays, keeping the join from its keys: its
/// operand's rows are all kept, so no filter of them is its.
pub struct PushFilterIntoOuterJoin;

impl Rule for PushFilterIntoOuterJoin {
    fn name(&self) -> &'static str {
        PUSH_FILTER_INTO_OUTER_JOIN
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        match plan {
            LogicalPlan::Filter { cond, input } => {
                let LogicalPlan::OuterJoin { kind, cond: on, left, right } = &**input else { return None };
                if cond.can_raise() {
                    return None;
                }
                let join = Join { kind: *kind, on, left, right };
                join.simplified(cond).or_else(|| join.preserved_pushed(cond))
            }
            LogicalPlan::OuterJoin { kind, cond: on, left, right } => Join { kind: *kind, on, left, right }.on_pushed(),
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Project { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Sort { .. } => None,
        }
    }
}

/// The parts of an outer join.
struct Join<'a> {
    kind: OuterJoin,
    on: &'a Cond,
    left: &'a LogicalPlan,
    right: &'a LogicalPlan,
}

/// An operand of a join.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// The operand whose columns `c` uses alone, the first `width` being the
/// left's; none if it uses both's, or none.
fn side(c: &Cond, width: usize) -> Option<Side> {
    let mut columns = Vec::new();
    cond_columns(c, &mut columns);
    if columns.is_empty() {
        None
    } else if columns.iter().all(|&i| i < width) {
        Some(Side::Left)
    } else if columns.iter().all(|&i| i >= width) {
        Some(Side::Right)
    } else {
        None
    }
}

/// Whether `c`, over the columns of one operand alone, is not true when
/// they are null: false or unknown, evaluated here on a row of nulls. One
/// whose evaluation fails, or needs a subquery, is not known to be.
fn rejects_nulls(c: &Cond, width: usize) -> bool {
    let nulls = vec![Value::Null; width];
    matches!(eval_cond(c, &nulls, &NoSubqueries), Ok(Truth::False | Truth::Unknown))
}

impl Join<'_> {
    fn width(&self) -> usize {
        self.left.width()
    }

    /// The join `kind` of these operands: an outer join on `on`, or, for
    /// none, the inner join, the filter over their product.
    fn of(&self, kind: Option<OuterJoin>, left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
        let (on, left, right) = (self.on.clone(), Box::new(left), Box::new(right));
        match kind {
            Some(kind) => LogicalPlan::OuterJoin { kind, cond: on, left, right },
            None => LogicalPlan::Filter { cond: on, input: Box::new(LogicalPlan::Product { left, right }) },
        }
    }

    /// The join `cond` filters, made the join it is if a conjunct over one
    /// operand's columns rejects the nulls that operand is given by the
    /// other's preservation; `None` if none does.
    fn simplified(&self, cond: &Cond) -> Option<LogicalPlan> {
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let width = self.left.width().saturating_add(self.right.width());
        let rejected = |s: Side| conjuncts.iter().any(|c| side(c, self.width()) == Some(s) && rejects_nulls(c, width));
        // The operands given nulls: the right by LEFT and FULL, the left by RIGHT and FULL.
        let (left_nulls, right_nulls) = (self.kind.keeps_right(), self.kind.keeps_left());
        let (no_left_nulls, no_right_nulls) = (left_nulls && rejected(Side::Left), right_nulls && rejected(Side::Right));
        if !no_left_nulls && !no_right_nulls {
            return None;
        }
        let keeps_left = self.kind.keeps_left() && !no_right_nulls;
        let keeps_right = self.kind.keeps_right() && !no_left_nulls;
        let kind = match (keeps_left, keeps_right) {
            (true, true) => Some(OuterJoin::Full),
            (true, false) => Some(OuterJoin::Left),
            (false, true) => Some(OuterJoin::Right),
            (false, false) => None,
        };
        Some(wrap(self.of(kind, self.left.clone(), self.right.clone()), conjuncts))
    }

    /// The join `cond` filters, its conjuncts over the preserved operand's
    /// columns alone, of a LEFT or RIGHT join, moved into that operand;
    /// `None` if there are none.
    fn preserved_pushed(&self, cond: &Cond) -> Option<LogicalPlan> {
        let preserved = match self.kind {
            OuterJoin::Left => Side::Left,
            OuterJoin::Right => Side::Right,
            OuterJoin::Full => return None,
        };
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (moved, kept): (Vec<Cond>, Vec<Cond>) = conjuncts.into_iter().partition(|c| side(c, self.width()) == Some(preserved));
        if moved.is_empty() {
            return None;
        }
        let join = self.with_moved(preserved, moved);
        Some(wrap(join, kept))
    }

    /// The join, its ON conjuncts over the columns of the operand a LEFT or
    /// RIGHT join gives nulls alone moved into that operand; of a condition
    /// that can raise an error, those that can (`raising_pushed`); `None`
    /// if there are none.
    fn on_pushed(&self) -> Option<LogicalPlan> {
        let nulls = match self.kind {
            OuterJoin::Left => Side::Right,
            OuterJoin::Right => Side::Left,
            OuterJoin::Full => return None,
        };
        let mut conjuncts = Vec::new();
        split_and(self.on.clone(), &mut conjuncts);
        if self.on.can_raise() {
            return self.raising_pushed(nulls, conjuncts);
        }
        let (moved, kept): (Vec<Cond>, Vec<Cond>) = conjuncts.into_iter().partition(|c| side(c, self.width()) == Some(nulls));
        if moved.is_empty() {
            return None;
        }
        // No conjunct left is a condition always true.
        let on = crate::logical::and_all(kept).unwrap_or(Cond::Const(Truth::True));
        Some(Join { on: &on, ..*self }.with_moved(nulls, moved))
    }

    /// The join, its ON `conjuncts` that can raise an error, each over the
    /// columns of `nulls`, the operand it gives nulls, alone, moved onto
    /// that operand over a guard of the other (`raising::guarded`); `None`
    /// if another can raise, or the other has an outer reference slot,
    /// which the guard's subquery would leave unfilled.
    fn raising_pushed(&self, nulls: Side, conjuncts: Vec<Cond>) -> Option<LogicalPlan> {
        let (raising, kept): (Vec<Cond>, Vec<Cond>) = conjuncts.into_iter().partition(Cond::can_raise);
        let (operand, other) = match nulls {
            Side::Left => (self.left, self.right),
            Side::Right => (self.right, self.left),
        };
        if raising.iter().any(|c| side(c, self.width()) != Some(nulls)) || plan_has_outer(other) {
            return None;
        }
        let width = self.width();
        let moved = match nulls {
            Side::Left => raising,
            Side::Right => raising.into_iter().map(|c| map_cond_columns(c, &|i| i.saturating_sub(width))).collect(),
        };
        let operand = guarded(0, moved, &[operand, other])?;
        let on = crate::logical::and_all(kept).unwrap_or(Cond::Const(Truth::True));
        let join = Join { on: &on, ..*self };
        Some(match nulls {
            Side::Left => join.of(Some(self.kind), operand, self.right.clone()),
            Side::Right => join.of(Some(self.kind), self.left.clone(), operand),
        })
    }

    /// The join, `moved`, conditions over `side`'s columns, applied to that
    /// operand first.
    fn with_moved(&self, side: Side, moved: Vec<Cond>) -> LogicalPlan {
        let width = self.width();
        let (left, right) = match side {
            Side::Left => (wrap(self.left.clone(), moved), self.right.clone()),
            Side::Right => {
                let shifted = moved.into_iter().map(|c| map_cond_columns(c, &|i| i.saturating_sub(width))).collect();
                (self.left.clone(), wrap(self.right.clone(), shifted))
            }
        };
        self.of(Some(self.kind), left, right)
    }
}
