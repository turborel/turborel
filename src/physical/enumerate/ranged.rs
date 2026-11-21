//! \[DESIGN\] Range joins in join enumeration (RULES.md D580, D303): which
//! conjuncts compare a value of two relations by `<`, `<=`, `>` or `>=`,
//! a `<period predicate>` of two of them among them (RULES.md D581), so a
//! join judging one may be a range join (`physical/range.rs`), costed with
//! the other methods (`enumerate/methods.rs`); and the plan of one chosen,
//! the operand of fewer rows sorted unless the left's order was chosen or
//! the join is LEFT, whose left rows must search. A join whose conditions
//! can raise by their columns' declared types, or whose search finds no
//! comparison in the end, is a nested loop instead.

use super::bits::Set;
use super::conflicts::Kind;
use super::extract::Query;
use super::model::Order;
use super::realize::{Context, Realized};
use super::space::Applied;
use crate::logical::{Cond, Expr, and_all, cond_can_raise_typed, expr_columns, split_and};
use crate::physical::range::{Made, expanded_periods, range_plan, ranged};
use crate::physical::shown::Shown;
use crate::physical::{CostModel, PhysicalPlanner};
use crate::settings::RANGE_JOINS;
use crate::value::CompOp;

/// Of each conjunct of the inner joins and each other operator's
/// predicate, whether it holds a comparison a range join searches by.
#[derive(Debug, Clone, Default)]
pub(super) struct Ranges {
    pub(super) conjuncts: Vec<bool>,
    pub(super) preds: Vec<bool>,
}

impl Ranges {
    /// Whether a join applying `applied` judges a comparison a range join
    /// searches by.
    pub(super) fn of(&self, applied: &Applied) -> bool {
        let of_pred = applied.op.and_then(|op| self.preds.get(op).copied()).unwrap_or(false);
        of_pred || applied.conjuncts.iter().any(|&c| self.conjuncts.get(c).copied().unwrap_or(false))
    }
}

/// The cost of a range join of operands of `rows` rows, the left's first,
/// giving `est`.
pub(super) fn range_join_cost(cost: &CostModel, rows: (f64, f64), est: f64) -> f64 {
    crate::physical::range::range_cost(cost, rows, (est, est), true)
}

impl PhysicalPlanner<'_> {
    /// The ranges of `q`'s conjuncts and predicates, if PLANNER.RANGE_JOINS
    /// is ON; none else.
    pub(super) fn ranges(&self, q: &Query) -> Ranges {
        if !self.db.settings.is_on(RANGE_JOINS) {
            return Ranges::default();
        }
        let conjuncts = q.conjuncts.iter().map(|(_, c)| crosses(c, q)).collect();
        let preds = q
            .preds
            .iter()
            .map(|p| {
                let mut parts = Vec::new();
                if let Some(c) = p {
                    split_and(c.clone(), &mut parts);
                }
                parts.iter().any(|c| crosses(c, q))
            })
            .collect();
        Ranges { conjuncts, preds }
    }

    /// The range join of `l` and `r`, of kind `kind`, judging `conds`, its
    /// operands giving `rows`, it `est`; none if it cannot be one.
    pub(super) fn range_realized(
        &self,
        cx: &Context<'_>,
        kind: Kind,
        (l, r): (&Realized, &Realized),
        (conds, rows, est): (&[Cond], (f64, f64), f64),
    ) -> Option<Realized> {
        let swap = kind == Kind::Inner && l.order == Order::None && rows.0 < rows.1;
        let (l, r) = if swap { (r, l) } else { (l, r) };
        let layout: Vec<usize> = l.layout.iter().chain(&r.layout).copied().collect();
        let mapped: Vec<Cond> = conds.iter().map(|c| cx.mapped(c, &layout)).collect();
        let full = and_all(mapped.clone())?;
        let types = |place: usize| {
            let col = (0..cx.query.width).find(|&c| cx.place_of(&layout, c) == Some(place))?;
            let leaf = cx.query.leaves.get(cx.query.rel_of(col)?)?;
            self.column_type(&leaf.plan, col.checked_sub(leaf.start)?)
        };
        if cond_can_raise_typed(&full, &types) {
            return None;
        }
        let width = cx.width(&l.layout);
        let found = ranged(expanded_periods(mapped), width)?;
        let names = cx.names(&layout, self.shown);
        let made = Made { outer: kind == Kind::Left, widths: (width, cx.width(&r.layout)), full, shown: Shown::of(names.as_deref()), est };
        let plan = range_plan(found, (l.plan.clone(), r.plan.clone()), made);
        let shape = if l.shape.is_empty() { String::new() } else { format!("({} {})", l.shape, r.shape) };
        Some(Realized { plan, layout, order: l.order, shape })
    }
}

/// Whether `c` is, or a `<period predicate>` it is made of holds, a
/// comparison by order of values each of a different relation of `q`.
fn crosses(c: &Cond, q: &Query) -> bool {
    expanded_periods(vec![c.clone()]).iter().any(|part| {
        let Cond::Compare(a, op, b) = part else { return false };
        let (ra, rb) = (relations(a, q), relations(b, q));
        !matches!(op, CompOp::Eq | CompOp::Ne) && ra != 0 && rb != 0 && ra & rb == 0
    })
}

/// The relations of `q` whose columns `e` reads.
fn relations(e: &Expr, q: &Query) -> Set {
    let mut columns = Vec::new();
    expr_columns(e, &mut columns);
    columns.iter().filter_map(|&col| q.rel_of(col)).fold(0, |s, r| s | super::bits::bit(r))
}
