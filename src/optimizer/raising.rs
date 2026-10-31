//! \[DESIGN\] Conditions that can raise an error moved into the operand of a
//! product whose columns they read, each still evaluated on every row of
//! the product it was (RULES.md D15). Split from optimizer.rs (RULES.md
//! R-SIZE-2).

use std::sync::Arc;

use super::{Rule, RuleContext, wrap};
use crate::logical::{Cond, LogicalPlan, Subquery, and_all, cond_columns, map_cond_columns, plan_has_outer, split_and};

/// See [`super::CONSTANT_FOLDING`].
pub const PUSH_RAISING_FILTER_INTO_PRODUCT: &str = "push_raising_filter_into_product";

/// A filter over a chain of products, some of whose conjuncts can raise an
/// error, each reading the columns of one operand of the chain alone: each
/// such conjunct is moved onto its operand, over a product of it with a
/// guard, a row of no columns if every other operand has a row, else
/// none (an EXISTS of each, over one row of none). The conjunct is then
/// evaluated on every row of its operand exactly when the chain's product
/// has rows, as over that product: there each row of the operand is in
/// some row of the product if, and only if, every other operand has a row
/// [X3.135-1986 §5.20 GR2b]. Of an operand that is empty, the conjunct is
/// evaluated on no row, as over the empty product. The conjuncts that
/// cannot raise an error stay over the chain, for
/// `PushFilterIntoProduct` to move, and an equality of two operands'
/// columns among them can then be a hash join's key, and a comparison of
/// another operand's column an index probe. A conjunct that can raise and
/// reads two operands' columns, or none, keeps the filter whole.
pub struct PushRaisingFilterIntoProduct;

impl Rule for PushRaisingFilterIntoProduct {
    fn name(&self) -> &'static str {
        PUSH_RAISING_FILTER_INTO_PRODUCT
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        if !cond.can_raise() || !matches!(**input, LogicalPlan::Product { .. }) {
            return None;
        }
        let mut operands = Vec::new();
        operands_of(input, &mut operands);
        // A guard has no columns: an operand of none is one, whose
        // conjuncts were moved already. An outer reference slot would be
        // left unfilled in the guard's subquery.
        if operands.iter().any(|o| o.width() == 0 || plan_has_outer(o)) {
            return None;
        }
        let starts: Vec<usize> = operands.iter().scan(0_usize, |at, o| Some(std::mem::replace(at, at.saturating_add(o.width())))).collect();
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (mut own, mut rest) = (vec![Vec::new(); operands.len()], Vec::new());
        for c in conjuncts {
            if !c.can_raise() {
                rest.push(c);
                continue;
            }
            let at = operand_of(&c, &starts, &operands)?;
            let start = starts.get(at).copied().unwrap_or_default();
            own.get_mut(at)?.push(map_cond_columns(c, &|i| i.saturating_sub(start)));
        }
        let mut moved = own.into_iter().enumerate().map(|(at, conds)| guarded(at, conds, &operands));
        let product = replaced(input, &mut moved)?;
        Some(wrap(product, rest))
    }
}

/// The operands of the chain of products `plan`, left to right.
fn operands_of<'p>(plan: &'p LogicalPlan, out: &mut Vec<&'p LogicalPlan>) {
    if let LogicalPlan::Product { left, right } = plan {
        operands_of(left, out);
        operands_of(right, out);
    } else {
        out.push(plan);
    }
}

/// The operand, of those beginning at `starts`, whose columns `c` reads
/// alone; none if it reads none, or those of two.
fn operand_of(c: &Cond, starts: &[usize], operands: &[&LogicalPlan]) -> Option<usize> {
    let mut columns = Vec::new();
    cond_columns(c, &mut columns);
    let of =
        |column: usize| starts.iter().zip(operands).position(|(&start, o)| column >= start && column < start.saturating_add(o.width()));
    let first = of(*columns.first()?)?;
    columns.iter().all(|&column| of(column) == Some(first)).then_some(first)
}

/// The operand `at` of `operands`, its conjuncts `conds` over a product of
/// it with the guard of the others, if it has any.
pub(super) fn guarded(at: usize, conds: Vec<Cond>, operands: &[&LogicalPlan]) -> Option<LogicalPlan> {
    let operand = (*operands.get(at)?).clone();
    let Some(cond) = and_all(conds) else { return Some(operand) };
    let others = operands.iter().enumerate().filter(|(n, _)| *n != at);
    let exists = others.map(|(_, o)| Cond::Exists(Arc::new(Subquery { plan: (*o).clone(), outer: Vec::new() }))).collect();
    let row = LogicalPlan::Values { rows: vec![Vec::new()], width: 0 };
    let guard = wrap(row, exists);
    Some(LogicalPlan::Filter { cond, input: Box::new(LogicalPlan::Product { left: Box::new(operand), right: Box::new(guard) }) })
}

/// The chain of products `plan`, its operands replaced, left to right, by
/// those `moved` gives.
fn replaced(plan: &LogicalPlan, moved: &mut impl Iterator<Item = Option<LogicalPlan>>) -> Option<LogicalPlan> {
    if let LogicalPlan::Product { left, right } = plan {
        let l = replaced(left, moved)?;
        let r = replaced(right, moved)?;
        return Some(LogicalPlan::Product { left: Box::new(l), right: Box::new(r) });
    }
    moved.next().flatten()
}

/// Whether `plan` is an operand moved by the rule: a product of it with a
/// guard of no columns, under its conjuncts. Join ordering takes it as one
/// operand (`physical/order.rs`).
pub(crate) fn is_guarded(plan: &LogicalPlan) -> bool {
    let LogicalPlan::Filter { input, .. } = plan else { return false };
    let LogicalPlan::Product { right, .. } = &**input else { return false };
    right.width() == 0
}

// Unit tests: which conjuncts move, and the plans the rule refuses
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MemoryCatalog;
    use crate::db::State;
    use crate::logical::Expr;
    use crate::value::{ArithOp, CompOp, Decimal, Value};

    /// An operand of one row of `width` columns.
    fn operand(width: usize) -> LogicalPlan {
        LogicalPlan::Values { rows: vec![vec![Expr::Const(Value::Exact(Decimal::new(1, 0))); width]], width }
    }

    fn product(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Product { left: Box::new(left), right: Box::new(right) }
    }

    /// "100 / (a - b) > 0", which can raise an error.
    fn raising(a: Expr, b: Expr) -> Cond {
        let quotient = Expr::Arith(
            Box::new(Expr::Const(Value::Exact(Decimal::new(100, 0)))),
            ArithOp::Div,
            Box::new(Expr::Arith(Box::new(a), ArithOp::Sub, Box::new(b))),
        );
        Cond::Compare(quotient, CompOp::Gt, Expr::Const(Value::Exact(Decimal::new(0, 0))))
    }

    /// The rule applied to a filter of `cond` over `input`.
    fn applied(cond: Cond, input: LogicalPlan) -> Option<LogicalPlan> {
        let st = State::with_builtins(Box::new(MemoryCatalog::default()));
        PushRaisingFilterIntoProduct.apply(&LogicalPlan::Filter { cond, input: Box::new(input) }, &RuleContext { db: &st })
    }

    /// Verifies: [X3.135-1986 §5.20 GR2b]; [DESIGN] RULES.md D15: a
    /// conjunct that can raise an error and reads one operand of three
    /// moves onto it, over a guard of the other two, and the equality of
    /// two operands stays over the product; one reading two operands, or
    /// none, an operand of no columns or one with an outer reference slot,
    /// and a filter that cannot raise or is not over a product, are left.
    #[test]
    fn a_raising_conjunct_moves_onto_its_operand() {
        let chain = || product(product(operand(2), operand(1)), operand(1));
        let equal = Cond::Compare(Expr::Column(1), CompOp::Eq, Expr::Column(2));
        let cond = Cond::And(Box::new(equal.clone()), Box::new(raising(Expr::Column(3), Expr::Column(3))));
        let Some(LogicalPlan::Filter { cond: kept, input }) = applied(cond, chain()) else { panic!("moved") };
        assert!(!kept.can_raise());
        let LogicalPlan::Product { right, .. } = *input else { panic!("{input:?}") };
        assert!(is_guarded(&right), "{right:?}");
        let LogicalPlan::Filter { cond: moved, input: guarded } = *right else { panic!("guarded") };
        assert!(moved.can_raise() && matches!(*guarded, LogicalPlan::Product { .. }));
        assert!(!is_guarded(&chain()) && !is_guarded(&LogicalPlan::Filter { cond: equal.clone(), input: Box::new(operand(1)) }));
        let only_raising = applied(raising(Expr::Column(0), Expr::Column(1)), chain());
        assert!(matches!(only_raising, Some(LogicalPlan::Product { .. })), "{only_raising:?}");
        let refused = [
            (raising(Expr::Column(0), Expr::Column(2)), chain()),
            (raising(Expr::Const(Value::Exact(Decimal::new(1, 0))), Expr::Const(Value::Exact(Decimal::new(1, 0)))), chain()),
            (equal.clone(), chain()),
            (raising(Expr::Column(0), Expr::Column(0)), operand(2)),
            (raising(Expr::Column(0), Expr::Column(0)), product(operand(2), operand(0))),
            (
                raising(Expr::Column(0), Expr::Column(0)),
                product(
                    operand(1),
                    LogicalPlan::Filter {
                        cond: Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Outer(0, "X".into())),
                        input: Box::new(operand(1)),
                    },
                ),
            ),
        ];
        for (condition, below) in refused {
            assert!(applied(condition.clone(), below).is_none(), "{condition:?}");
        }
    }
}
