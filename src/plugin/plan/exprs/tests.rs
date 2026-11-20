//! Unit tests: the expressions of a form a rule gave, made the planner's
//! and checked, each kind accepted and each not valid refused, SQLSTATE
//! 38T03 (RULES.md R-TEST-1, R-TEST-2).

use crate::error::Result;
use crate::logical::{Cond, Expr, LogicalPlan, Truth};
use crate::plugin::plan::back::tests::{Given, made, refused, scan};
use crate::plugin::plan::{Body, Expression, Op, Operator};
use crate::value::{CompOp, Decimal, Value};

/// The column at `i`.
fn col(i: u32) -> Expression {
    Expression::Column(i)
}

/// An INTEGER literal of `n`.
fn int(n: i128) -> Expression {
    Expression::Literal { data_type: "INTEGER".into(), value: Value::Exact(Decimal::new(n, 0)) }
}

/// A BOOLEAN literal of `b`.
fn boolean(b: bool) -> Expression {
    Expression::Literal { data_type: "BOOLEAN".into(), value: Value::Bool(b) }
}

/// `op` of `args`.
fn op(op: Operator, args: Vec<Expression>) -> Expression {
    Expression::Operator { op, args }
}

/// A filter of U.T, A, B and C, on B IS DISTINCT FROM A and on a CASE
/// greater than 1: two expressions the form shows opaque, the first a
/// condition, the second a value.
fn plan() -> LogicalPlan {
    let case = Expr::Case(vec![(Cond::Const(Truth::True), Expr::Column(1))], Box::new(Expr::Column(0)));
    let one = Expr::Const(Value::Exact(Decimal::new(1, 0)));
    let cond = Cond::And(Box::new(Cond::Distinct(Expr::Column(1), Expr::Column(0))), Box::new(Cond::Compare(case, CompOp::Gt, one)));
    LogicalPlan::Filter { cond, input: Box::new(scan()) }
}

/// The form of `plan`.
fn given() -> Given {
    Given::of(&plan())
}

/// The plan of a filter of U.T on `cond`, taken back for `g`'s.
fn filter(g: &Given, cond: Expression) -> Result<LogicalPlan> {
    g.back(made(Op::Filter, Body::Filter(cond), g.form.root.inputs.clone()))
}

/// Verifies: [DESIGN] RULES.md D275, D277: a form's filter given back
/// unchanged makes the plan again; its opaque expressions are taken back
/// over the columns a rule names, a condition's used as a value and a
/// value's as a condition.
#[test]
fn opaque_expressions_are_taken_back_named_again() {
    let g = given();
    assert!(matches!(&g.form.root.body, Body::Filter(Expression::Operator { op: Operator::And, .. })));
    assert_eq!(g.back(g.form.root.clone()).unwrap().explain(), plan().explain());
    let shown = filter(&g, Expression::Opaque { id: 1, columns: vec![1, 2] }).unwrap().explain();
    assert!(shown.contains("DISTINCT"), "{shown}");
    filter(&g, op(Operator::Eq, vec![Expression::Opaque { id: 1, columns: vec![1, 0] }, boolean(true)])).unwrap();
    filter(&g, Expression::Opaque { id: 2, columns: vec![0, 1] }).unwrap();
}

/// Verifies: [DESIGN] RULES.md D277: an opaque expression the form did
/// not keep, one used twice, or one over another count of columns, or a
/// column past the row, is refused.
#[test]
fn opaque_expressions_not_kept_are_refused() {
    let g = given();
    let opaque = |id, columns: Vec<u32>| Expression::Opaque { id, columns };
    refused(filter(&g, opaque(0, vec![1, 0])), "expression 0");
    refused(filter(&g, opaque(3, vec![1, 0])), "expression 3 of 2");
    refused(filter(&g, op(Operator::And, vec![opaque(1, vec![1, 0]), opaque(1, vec![1, 0])])), "used twice");
    refused(filter(&g, opaque(1, vec![1])), "one column of two");
    refused(filter(&g, opaque(1, vec![1, 9])), "column 9 of 3");
}

/// Verifies: [DESIGN] RULES.md D277 [X3.135-1986 §5.9, §5.11, §5.15,
/// §5.18]: each comparison, AND, OR, NOT, IS NULL, IS NOT NULL, dyadic
/// arithmetic and negation, a function by its name, CAST, a BOOLEAN
/// literal, the null value of no type, a truth value as a value, and a
/// literal CAST to the type it names, are taken back as conditions.
#[test]
fn each_expression_a_rule_writes_is_taken_back() {
    let g = given();
    let compared = [Operator::Eq, Operator::Ne, Operator::Lt, Operator::Gt, Operator::Le, Operator::Ge];
    for c in compared {
        filter(&g, op(c, vec![col(0), int(3)])).unwrap();
    }
    for a in [Operator::Add, Operator::Sub, Operator::Mul, Operator::Div] {
        filter(&g, op(Operator::Gt, vec![op(a, vec![col(0), col(1)]), int(1)])).unwrap();
    }
    let both = op(Operator::Or, vec![op(Operator::IsNull, vec![col(0)]), op(Operator::Not, vec![op(Operator::IsNotNull, vec![col(2)])])]);
    let shown = filter(&g, op(Operator::And, vec![both, boolean(true)])).unwrap().explain();
    assert!(shown.contains("OR") && shown.contains("NOT"), "{shown}");
    filter(&g, op(Operator::Lt, vec![op(Operator::Negate, vec![col(0)]), int(0)])).unwrap();
    filter(&g, op(Operator::Gt, vec![Expression::Call { name: "FLOOR".into(), args: vec![col(0)] }, int(1)])).unwrap();
    let cast = Expression::Cast { data_type: "BIGINT".into(), arg: Box::new(col(1)) };
    filter(&g, op(Operator::Ge, vec![cast, int(1)])).unwrap();
    filter(&g, Expression::Literal { data_type: String::new(), value: Value::Null }).unwrap();
    filter(&g, Expression::Cast { data_type: "BOOLEAN".into(), arg: Box::new(boolean(false)) }).unwrap();
    filter(&g, op(Operator::Eq, vec![op(Operator::IsNull, vec![col(0)]), boolean(false)])).unwrap();
    let numeric = Expression::Literal { data_type: "NUMERIC(1)".into(), value: Value::Exact(Decimal::new(3, 0)) };
    filter(&g, op(Operator::Eq, vec![col(0), numeric])).unwrap();
}

/// Verifies: [DESIGN] RULES.md D277 [X3.135-1986 §5.9 SR3, §5.11 SR1]:
/// a column past the row, an operator of the wrong count of operands, a
/// string an operand of arithmetic, a CAST to no type, a function no one
/// name finds or of arguments it does not take, a comparison of types not
/// comparable, a condition of a value or literal not BOOLEAN, and a
/// literal of no type or not of its type, are each refused.
#[test]
fn each_expression_not_valid_is_refused() {
    let g = given();
    let gt = |x: Expression| op(Operator::Gt, vec![x, int(1)]);
    let chr = Expression::Literal { data_type: "CHARACTER(1)".into(), value: Value::Char("x".into()) };
    let cases = [
        ("column 3 of 3", gt(col(3))),
        ("+ of one", gt(op(Operator::Add, vec![col(0)]))),
        ("negation of two", gt(op(Operator::Negate, vec![col(0), col(1)]))),
        ("a string added", gt(op(Operator::Add, vec![col(2), int(1)]))),
        ("a string negated", gt(op(Operator::Negate, vec![col(2)]))),
        ("a CAST to no type", gt(Expression::Cast { data_type: String::new(), arg: Box::new(col(0)) })),
        ("no function", gt(Expression::Call { name: "NO_SUCH_FUNCTION".into(), args: Vec::new() })),
        ("FLOOR of a string", gt(Expression::Call { name: "FLOOR".into(), args: vec![chr.clone()] })),
        ("= of three", op(Operator::Eq, vec![col(0), col(1), col(0)])),
        ("INTEGER = CHARACTER", op(Operator::Eq, vec![col(0), col(2)])),
        ("AND of one", op(Operator::And, vec![boolean(true)])),
        ("NOT of two", op(Operator::Not, vec![boolean(true), boolean(false)])),
        ("IS NULL of none", op(Operator::IsNull, Vec::new())),
        ("an INTEGER column", col(0)),
        ("an INTEGER sum", op(Operator::Add, vec![col(0), col(1)])),
        ("an INTEGER literal", int(1)),
        ("a value of no type", gt(Expression::Literal { data_type: String::new(), value: Value::Bool(true) })),
        ("'x' an INTEGER", gt(Expression::Literal { data_type: "INTEGER".into(), value: Value::Char("x".into()) })),
    ];
    for (what, cond) in cases {
        refused(filter(&g, cond), what);
    }
    let unparsed = gt(Expression::Cast { data_type: "NO TYPE AT ALL".into(), arg: Box::new(col(0)) });
    assert!(filter(&g, unparsed).is_err(), "a CAST to text no type is");
}
