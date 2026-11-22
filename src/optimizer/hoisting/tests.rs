//! Unit tests: which parts the rule takes, onto which operand, over a
//! guard or not, and the plans it leaves (RULES.md R-TEST-1, R-TEST-8).

use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::logical::{OuterJoin, Truth};
use crate::value::collation::{CASE_INSENSITIVE, CollationRef, Pad, builtin};
use crate::value::{ArithOp, CompOp, DataType, Decimal, Spelled, Value};

/// An operand of one row of `width` INTEGER columns.
fn operand(width: usize) -> LogicalPlan {
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    LogicalPlan::Values { rows: vec![vec![one(); width]], types: vec![integer; width] }
}

fn product(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
    LogicalPlan::Product { left: Box::new(left), right: Box::new(right) }
}

fn arith(a: Expr, op: ArithOp, b: Expr) -> Expr {
    Expr::Arith(Box::new(a), op, Box::new(b))
}

fn one() -> Expr {
    Expr::Const(Value::Exact(Decimal::new(1, 0)))
}

/// The sort key of `e` under the collation CASE_INSENSITIVE.
fn key(e: Expr) -> Expr {
    let collation = builtin().into_iter().find(|c| c.name() == CASE_INSENSITIVE).unwrap();
    Expr::SortKey(Box::new(e), CollationRef { collation, pad: Pad::Space })
}

/// The equality of the sort keys of columns `a` and `b`.
fn keyed(a: usize, b: usize) -> Cond {
    Cond::Compare(key(Expr::Column(a)), CompOp::Eq, key(Expr::Column(b)))
}

fn greater(a: Expr, b: Expr) -> Cond {
    Cond::Compare(a, CompOp::Gt, b)
}

/// The rule applied to `plan`.
fn applied(plan: &LogicalPlan) -> Option<LogicalPlan> {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    HoistRaisingExpressions.apply(plan, &RuleContext { db: &st })
}

/// The filter of `cond` over `input`, hoisted.
fn filtered(cond: Cond, input: LogicalPlan) -> Option<LogicalPlan> {
    applied(&LogicalPlan::Filter { cond, input: Box::new(input) })
}

/// The operands of the chain under the projection and filter the rule
/// made, and the filter's condition.
fn operands_made(plan: LogicalPlan) -> (Vec<LogicalPlan>, Cond) {
    let LogicalPlan::Project { input, .. } = plan else { panic!("{plan:?}") };
    let LogicalPlan::Filter { cond, input } = *input else { panic!("filtered") };
    let mut out = Vec::new();
    operands_of(&input, &mut out);
    (out.into_iter().cloned().collect(), cond)
}

/// Whether `plan` is an operand with parts after its columns over a
/// guard of the others.
fn guarded(plan: &LogicalPlan) -> bool {
    let LogicalPlan::Project { input, .. } = plan else { return false };
    let LogicalPlan::Product { right, .. } = &**input else { return false };
    right.width() == 0
}

/// Verifies: [X3.135-1986 §5.20 GR2b]; [DESIGN] RULES.md D373, D15: of
/// `A.X / A.Y > B.Z`, the quotient, which can raise and reads A alone, is
/// a column after A's over a guard of B, and the conjunct reads it and no
/// longer raises; of a collated equality, each sort key is its operand's
/// column with no guard; of `A.X + 1 > B.Z`, the sum, which cannot raise of
/// INTEGER columns, is left; the projection over the chain keeps its
/// columns.
#[test]
fn parts_are_columns_of_their_operands() {
    let quotient = greater(arith(Expr::Column(0), ArithOp::Div, Expr::Column(1)), Expr::Column(2));
    let Some(plan) = filtered(quotient, product(operand(2), operand(1))) else { panic!("hoisted") };
    let LogicalPlan::Project { exprs, .. } = &plan else { panic!("{plan:?}") };
    assert_eq!(format!("{exprs:?}"), format!("{:?}", [Expr::Column(0), Expr::Column(1), Expr::Column(3)]));
    let (operands, cond) = operands_made(plan);
    assert!(guarded(&operands[0]) && operands[0].width() == 3, "{:?}", operands[0]);
    assert!(!cond.can_raise(), "{cond:?}");
    let (unguarded, _) = operands_made(filtered(keyed(0, 2), product(operand(2), operand(1))).unwrap());
    assert!(unguarded.iter().all(|o| !guarded(o) && matches!(o, LogicalPlan::Project { .. })), "{unguarded:?}");
    let sum = greater(arith(Expr::Column(0), ArithOp::Add, one()), Expr::Column(2));
    assert!(filtered(sum, product(operand(2), operand(1))).is_none());
}

/// Verifies: [ISO/IEC 9075:1992 §6.9 GR1, GR2]; [DESIGN] RULES.md D373:
/// the rule takes nothing of a conjunct whose raising part reads two
/// operands, of a CASE's result, or of a conjunct of one operand, nor of a
/// filter over no product; and nothing needing a guard when an operand
/// has an outer reference.
///
/// Decision `guard needed && an operand has an outer reference` in
/// `Hoisted::of`. MC/DC: (T T) refused; (T F) taken; (F T) taken.
#[test]
fn what_cannot_be_taken_is_left() {
    let chain = || product(operand(2), operand(1));
    let both = greater(arith(Expr::Column(0), ArithOp::Div, Expr::Column(2)), one());
    assert!(filtered(both, chain()).is_none(), "of two operands");
    let case =
        Expr::Case(vec![(greater(Expr::Column(2), one()), arith(Expr::Column(0), ArithOp::Div, Expr::Column(1)))].into(), Box::new(one()));
    assert!(filtered(greater(case, Expr::Column(2)), chain()).is_none(), "a CASE's result");
    let alone = greater(arith(Expr::Column(0), ArithOp::Div, Expr::Column(1)), one());
    assert!(filtered(alone, chain()).is_none(), "of one operand, which the raising rule moves");
    assert!(filtered(Cond::Const(Truth::True), operand(1)).is_none());
    let outer =
        LogicalPlan::Filter { cond: Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Outer(0, "X".into())), input: Box::new(operand(1)) };
    let quotient = || greater(arith(Expr::Column(0), ArithOp::Div, Expr::Column(1)), Expr::Column(2));
    assert!(filtered(quotient(), product(operand(2), outer.clone())).is_none(), "T T");
    assert!(filtered(quotient(), product(operand(2), operand(1))).is_some(), "T F");
    assert!(filtered(keyed(0, 2), product(operand(2), outer)).is_some(), "F T");
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1c, GR5]; [DESIGN] RULES.md D373:
/// of a LEFT join's ON, a raising part of the operand that supplies nulls
/// is taken over a guard; a raising part of the preserved one is not,
/// and of a FULL join neither is; a sort key, which cannot raise, is
/// taken of either; a sum that cannot raise is left.
///
/// Decision `supplies(at) || safe(at, e)` in `Join::hoisted`. MC/DC:
/// (F F) left; (T F) taken; (F T) taken.
#[test]
fn outer_joins_take_parts_of_the_operand_that_supplies_nulls() {
    let join = |kind: OuterJoin, cond: Cond| LogicalPlan::OuterJoin { kind, cond, left: Box::new(operand(1)), right: Box::new(operand(2)) };
    let of_right = greater(arith(Expr::Column(1), ArithOp::Div, Expr::Column(2)), Expr::Column(0));
    let of_left = greater(arith(one(), ArithOp::Div, Expr::Column(0)), Expr::Column(1));
    let safe_left = keyed(0, 1);
    assert!(applied(&join(OuterJoin::Left, of_left.clone())).is_none(), "F F");
    let Some(LogicalPlan::Project { input, .. }) = applied(&join(OuterJoin::Left, of_right.clone())) else { panic!("T F") };
    let LogicalPlan::OuterJoin { right, cond, .. } = *input else { panic!("joined") };
    assert!(guarded(&right) && !cond.can_raise(), "{right:?}");
    assert!(applied(&join(OuterJoin::Left, safe_left.clone())).is_some(), "F T");
    assert!(applied(&join(OuterJoin::Right, of_left)).is_some(), "the left supplies nulls");
    assert!(applied(&join(OuterJoin::Full, of_right)).is_none());
    assert!(applied(&join(OuterJoin::Full, safe_left)).is_some());
    let sum = greater(arith(Expr::Column(0), ArithOp::Add, one()), Expr::Column(1));
    assert!(applied(&join(OuterJoin::Left, sum)).is_none());
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1c]; [DESIGN] RULES.md D373: a
/// LEFT join of a chain of products, through a projection of its columns,
/// whose condition reads one operand of it, is that operand's LEFT join
/// within the chain, its columns in their order; one reading two operands,
/// one that can raise, and a RIGHT join are left.
#[test]
fn left_joins_move_onto_the_operand_they_read() {
    let chain = || {
        let exprs = vec![Expr::Column(2), Expr::Column(0), Expr::Column(1)];
        LogicalPlan::Project { exprs, input: Box::new(product(operand(2), operand(1))) }
    };
    let join = |kind: OuterJoin, cond: Cond| LogicalPlan::OuterJoin { kind, cond, left: Box::new(chain()), right: Box::new(operand(1)) };
    let on_second = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Column(3));
    let Some(LogicalPlan::Project { input, .. }) = applied(&join(OuterJoin::Left, on_second.clone())) else { panic!("moved") };
    let LogicalPlan::Product { left, right } = *input else { panic!("a product") };
    assert!(matches!(*left, LogicalPlan::Values { .. }) && matches!(*right, LogicalPlan::OuterJoin { .. }), "{right:?}");
    let on_both = Cond::And(Box::new(on_second.clone()), Box::new(Cond::Compare(Expr::Column(1), CompOp::Eq, Expr::Column(3))));
    assert!(applied(&join(OuterJoin::Left, on_both)).is_none());
    let raising = Cond::Compare(arith(Expr::Column(0), ArithOp::Div, Expr::Column(3)), CompOp::Eq, one());
    assert!(applied(&join(OuterJoin::Left, raising)).is_none());
    assert!(applied(&join(OuterJoin::Right, on_second)).is_none());
}
