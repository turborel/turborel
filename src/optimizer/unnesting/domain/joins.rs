//! \[DESIGN\] The dependent join with D pushed down joins and set
//! operators (Neumann and Kemper, "Unnesting Arbitrary Queries", 2015,
//! §3.2).
//!
//! Of a product, D goes to the side that reads outer references, or, if
//! both do, to both, the two then joined on D as well. Of an outer join,
//! D goes to the preserved side alone if only it reads them; else to both,
//! joined on D in the join condition, so a preserved row that matches
//! none keeps its binding, a FULL join's binding that of whichever side
//! the row has. A join condition that can raise an error is left as it
//! is, as it would then be judged on pairs of two bindings. A set
//! operator's operands both take D, and compare its columns with the
//! rest, as D ⋈ (T1 ∪ T2) is (D ⋈ T1) ∪ (D ⋈ T2), and so of EXCEPT and
//! INTERSECT.

use super::{Domain, same, shifted};
use crate::logical::{Cond, Expr, LogicalPlan, OuterJoin, SetOp, and_all, map_cond_columns, plan_has_outer};

impl Domain {
    /// The product of `left` and `right`.
    pub(super) fn product(&self, left: &LogicalPlan, right: &LogicalPlan) -> Option<LogicalPlan> {
        let (by, lw, rw) = (self.width, left.width(), right.width());
        match (plan_has_outer(left), plan_has_outer(right)) {
            (true, false) => Some(LogicalPlan::Product { left: Box::new(self.push(left)?), right: Box::new(right.clone()) }),
            (false, true) => {
                let product = LogicalPlan::Product { left: Box::new(left.clone()), right: Box::new(self.push(right)?) };
                // D's columns, then the left's, then the right's.
                let order = (lw..lw.saturating_add(by)).chain(0..lw).chain(lw.saturating_add(by)..lw.saturating_add(by).saturating_add(rw));
                Some(LogicalPlan::Project { exprs: order.map(Expr::Column).collect(), input: Box::new(product) })
            }
            (true, true) | (false, false) => {
                let product = LogicalPlan::Product { left: Box::new(self.over(left)?), right: Box::new(self.over(right)?) };
                let joined = LogicalPlan::Filter { cond: self.on(lw), input: Box::new(product) };
                Some(self.dropped(joined, lw, rw))
            }
        }
    }

    /// Of a row of D, a side `lw` wide, D's copy and the other side, that
    /// the two copies of D are not distinct.
    fn on(&self, lw: usize) -> Cond {
        let at = self.width.saturating_add(lw);
        and_all((0..self.width).map(|i| same(i, at.saturating_add(i))).collect()).unwrap_or(Cond::Const(crate::logical::Truth::True))
    }

    /// `joined`, rows of D, a side `lw` wide, D's copy and a side `rw`
    /// wide, without the copy.
    fn dropped(&self, joined: LogicalPlan, lw: usize, rw: usize) -> LogicalPlan {
        let by = self.width;
        let right = by.saturating_mul(2).saturating_add(lw);
        let order = (0..by.saturating_add(lw)).chain(right..right.saturating_add(rw));
        LogicalPlan::Project { exprs: order.map(Expr::Column).collect(), input: Box::new(joined) }
    }

    /// An outer join of `kind` on `cond`.
    pub(super) fn outer_join(&self, (kind, cond): (OuterJoin, &Cond), left: &LogicalPlan, right: &LogicalPlan) -> Option<LogicalPlan> {
        if cond.can_raise() {
            return None;
        }
        let (by, lw, rw) = (self.width, left.width(), right.width());
        let on_reads = {
            let mut found = false;
            cond.for_each_expr(&mut |e| found = found || reads_outer(e));
            found
        };
        let one_side = !on_reads && kind == OuterJoin::Left && !plan_has_outer(right);
        if one_side {
            let cond = cond.clone().map_exprs(&mut |e| shifted(e, by));
            return Some(LogicalPlan::OuterJoin { kind, cond, left: Box::new(self.push(left)?), right: Box::new(right.clone()) });
        }
        // Both sides take D. The left's columns follow D, the right's D's copy.
        let (l, r) = (self.over(left)?, self.over(right)?);
        let copy = by.saturating_add(lw);
        let slot = |i: usize| if kind == OuterJoin::Right { copy.saturating_add(i) } else { i };
        let moved = map_cond_columns(cond.clone(), &|c| if c < lw { c.saturating_add(by) } else { c.saturating_add(by.saturating_mul(2)) });
        let moved = moved.map_exprs(&mut |e| bound_slots(e, &slot));
        let joined =
            LogicalPlan::OuterJoin { kind, cond: Cond::And(Box::new(moved), Box::new(self.on(lw))), left: Box::new(l), right: Box::new(r) };
        let right_at = by.saturating_mul(2).saturating_add(lw);
        let rest = (by..copy).chain(right_at..right_at.saturating_add(rw)).map(Expr::Column);
        let bindings: Vec<Expr> = (0..by)
            .map(|i| match kind {
                OuterJoin::Left => Expr::Column(i),
                OuterJoin::Right => Expr::Column(copy.saturating_add(i)),
                // Of the side the row has, by COALESCE.
                OuterJoin::Full => Expr::Case(
                    vec![(Cond::IsNull(Expr::Column(i), false), Expr::Column(copy.saturating_add(i)))],
                    Box::new(Expr::Column(i)),
                ),
            })
            .collect();
        Some(LogicalPlan::Project { exprs: bindings.into_iter().chain(rest).collect(), input: Box::new(joined) })
    }

    /// A set operator's operands, each D's dependent join.
    pub(super) fn set_op(
        &self,
        (op, all, compared): (SetOp, bool, Option<usize>),
        left: &LogicalPlan,
        right: &LogicalPlan,
    ) -> Option<LogicalPlan> {
        let compared = compared.map(|c| c.saturating_add(self.width));
        Some(LogicalPlan::SetOp { op, all, compared, left: Box::new(self.over(left)?), right: Box::new(self.over(right)?) })
    }
}

/// Whether `e` has an outer reference, among its subqueries' outer
/// expressions too.
fn reads_outer(e: &Expr) -> bool {
    let mut found = matches!(e, Expr::Outer(..));
    e.for_each_child(&mut |child| found = found || reads_outer(child));
    found
}

/// `e`, each outer reference the column `slot` gives of its slot.
fn bound_slots(e: Expr, slot: &dyn Fn(usize) -> usize) -> Expr {
    if let Expr::Outer(i, _) = e {
        return Expr::Column(slot(i));
    }
    e.map_children(&mut |x| bound_slots(x, slot))
}
