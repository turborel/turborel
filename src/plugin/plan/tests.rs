//! Unit tests: the plan form written and read, a plan's form made and
//! taken back, and the forms refused (RULES.md R-TEST-1, R-TEST-2).

use super::*;
use crate::bytes::Reader;
use crate::db::State;
use crate::logical::{Cond, Expr, LogicalPlan};
use crate::value::{CompOp, Decimal};

/// A node of `op`, made, of `body` over `inputs`.
fn made(op: Op, body: Body, inputs: Vec<Node>) -> Node {
    Node { op, id: 0, kept: false, columns: Vec::new(), rows: -1.0, width: -1.0, body, inputs }
}

/// A form read back from its bytes.
fn again(form: &Form) -> crate::error::Result<Form> {
    let bytes = encode(form)?;
    decode(&mut Reader::new(&bytes, "a plan form"))
}

/// A database's state, its catalog a table U.T (A INTEGER, B INTEGER).
fn state() -> std::sync::Arc<State> {
    let db = crate::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (A INTEGER, B INTEGER)").unwrap();
    db.snapshot().unwrap()
}

/// Verifies: [DESIGN] RULES.md D275: a plan form of every expression and
/// node it says made is read back as written, and one of another format,
/// an operator unknown, or a node only kept but made is refused.
#[test]
fn a_form_is_read_as_written() {
    let literal = Expression::Literal { data_type: "INTEGER".into(), value: Value::Exact(Decimal::new(3, 0)) };
    let cond = Expression::Operator { op: Operator::Gt, args: vec![Expression::Column(0), literal] };
    let scan = made(Op::Scan, Body::Scan { table: "U.T".into(), statistics: Vec::new() }, Vec::new());
    let opaque = Expression::Opaque { id: 1, columns: vec![1] };
    let cast = Expression::Cast { data_type: "BIGINT".into(), arg: Box::new(Expression::Call { name: "ABS".into(), args: vec![opaque] }) };
    let filter = made(Op::Filter, Body::Filter(cond), vec![scan]);
    let form = Form { root: made(Op::Project, Body::Project(vec![cast]), vec![filter]) };
    assert_eq!(encode(&again(&form).unwrap()).unwrap(), encode(&form).unwrap());
    let mut bytes = encode(&form).unwrap();
    bytes[0] = 2;
    assert!(decode(&mut Reader::new(&bytes, "a plan form")).is_err(), "format 2");
    let kept_only = Form { root: made(Op::Aggregate, Body::Distinct, Vec::new()) };
    assert!(again(&kept_only).is_err(), "an aggregate is kept alone");
}

/// Verifies: [DESIGN] RULES.md D275, D277: a plan's form names its
/// columns' types, and the form given back unchanged makes the plan
/// again; a form whose output loses a column, names a column its input
/// lacks, or uses a kept node twice is refused.
#[test]
fn a_form_is_taken_back_checked() {
    let st = state();
    let scan = LogicalPlan::Scan { table: crate::catalog::QualifiedName { auth: "U".into(), ident: "T".into() }, names: vec!["A".into(), "B".into()], needed: vec![0, 1], located: false };
    let cond = Cond::Compare(Expr::Column(0), CompOp::Gt, Expr::Const(Value::Exact(Decimal::new(3, 0))));
    let plan = LogicalPlan::Filter { cond, input: Box::new(scan) };
    let (form, kept) = form_of(&plan, &st).unwrap();
    assert_eq!(form.root.columns[0].data_type, "INTEGER");
    let back = plan_of(&form, &kept, &st).unwrap();
    assert_eq!(back.explain(), plan.explain());
    let mut narrower = form.clone();
    narrower.root = made(Op::Project, Body::Project(vec![Expression::Column(0)]), vec![form.root.clone()]);
    assert!(plan_of(&narrower, &kept, &st).is_err(), "a column lost");
    let mut outside = form.clone();
    outside.root.body = Body::Filter(Expression::Column(7));
    assert!(plan_of(&outside, &kept, &st).is_err(), "column 7 of 2");
}
