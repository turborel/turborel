//! Unit tests: every body and expression of the plan form written and
//! read back, and the forms a program could send that are refused, cut
//! short, of a code no operator has, or nested too deep (RULES.md
//! R-TEST-1, R-TEST-2).

use super::*;
use crate::error::SqlState;
use crate::plugin::errors::VIOLATION;
use crate::plugin::payload::reader;
use crate::value::{Binary, Decimal, Value};

/// A node of `op`, made, of `body` over `inputs`, with a column and
/// estimates known.
fn made(op: Op, body: Body, inputs: Vec<Node>) -> Node {
    let columns = vec![Column { name: "A".into(), data_type: "INTEGER".into() }];
    Node { op, id: 0, kept: false, columns, rows: 12.0, width: 4.0, body, inputs }
}

/// An INTEGER literal of `n`.
fn int(n: i128) -> Expression {
    Expression::Literal { data_type: "INTEGER".into(), value: Value::Exact(Decimal::new(n, 0)) }
}

/// An expression of every expression code and every operator.
fn every_expression() -> Expression {
    let mut args: Vec<Expression> =
        Operator::ALL.into_iter().map(|op| Expression::Operator { op, args: vec![Expression::Column(0), int(1)] }).collect();
    args.push(Expression::Cast { data_type: "BIGINT".into(), arg: Box::new(Expression::Column(1)) });
    args.push(Expression::Opaque { id: 1, columns: vec![0, 2] });
    args.push(Expression::Literal { data_type: String::new(), value: Value::Null });
    Expression::Call { name: "COALESCE".into(), args }
}

/// A form of every body: a kept node, scans with and without bounds,
/// filter, project, joins of each kind, sort, distinct, each set
/// operator, and values.
fn every_body() -> Form {
    let bounded = ColumnStatistics { distinct: 20.0, nulls: 0.25, bounds: Some((Value::Exact(Decimal::new(1, 0)), int_value(20))) };
    let unknown = ColumnStatistics { distinct: -1.0, nulls: -1.0, bounds: None };
    let scan = made(Op::Scan, Body::Scan { table: "U.T".into(), statistics: vec![bounded, unknown] }, Vec::new());
    let kept = Node { kept: true, id: 7, body: Body::Kept("Aggregate".into()), ..made(Op::Aggregate, Body::Distinct, vec![scan.clone()]) };
    let filter = made(Op::Filter, Body::Filter(every_expression()), vec![kept]);
    let values = made(Op::Values, Body::Values(vec![vec![int(1)], vec![int(2)]]), Vec::new());
    let mut joined = made(Op::Join, Body::Join { kind: JoinKind::Cross, condition: None }, vec![filter, values]);
    for kind in [JoinKind::Left, JoinKind::Right, JoinKind::Full] {
        let cond = Expression::Operator { op: Operator::Eq, args: vec![Expression::Column(0), Expression::Column(1)] };
        joined = made(Op::Join, Body::Join { kind, condition: Some(cond) }, vec![joined, scan.clone()]);
    }
    let keys = vec![Key { column: 0, descending: true, nulls_first: false }, Key { column: 1, descending: false, nulls_first: true }];
    let mut top = made(Op::Distinct, Body::Distinct, vec![made(Op::Sort, Body::Sort(keys), vec![joined])]);
    for (op, all) in [(SetKind::Union, true), (SetKind::Except, false), (SetKind::Intersect, true)] {
        top = made(Op::Set, Body::Set { op, all }, vec![top, scan.clone()]);
    }
    Form { root: made(Op::Project, Body::Project(vec![Expression::Column(0)]), vec![top]) }
}

/// An exact value of `n`.
fn int_value(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The form `bytes` hold, read as a program's result is.
fn read(bytes: &[u8]) -> Result<Form> {
    decode(&mut reader(bytes))
}

/// The SQLSTATE `bytes` are refused with.
fn refused(bytes: &[u8]) -> SqlState {
    read(bytes).unwrap_err().sqlstate()
}

/// A form's bytes up to its root's body: a node of operator `op`, made,
/// no columns, no estimates.
fn head(op: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.put_u32(FORMAT);
    out.put_u8(op);
    out.put_u32(0);
    out.put_u8(0);
    out.put_u32(0);
    put_f64(&mut out, -1.0);
    put_f64(&mut out, -1.0);
    out
}

/// Verifies: [DESIGN] RULES.md D275: a plan form of every body, join
/// kind, set operator, sort key, statistics with and without bounds,
/// expression code and operator is read back as written, byte for byte.
#[test]
fn every_body_and_expression_is_read_as_written() {
    let form = every_body();
    let bytes = encode(&form).unwrap();
    let back = read(&bytes).unwrap();
    assert_eq!(encode(&back).unwrap(), bytes);
    let mut ops = Vec::new();
    back.root.walk(&mut |n| ops.push(n.op));
    assert_eq!(ops.iter().filter(|o| **o == Op::Set).count(), 3);
    assert!(matches!(back.root.inputs[0].body, Body::Set { op: SetKind::Intersect, all: true }));
}

/// Verifies: [DESIGN] RULES.md D275, D172: a plan form cut short at any
/// byte is refused as a malformed message received, SQLSTATE 08006, and
/// never read as a shorter form.
#[test]
fn a_form_cut_short_is_refused_at_every_byte() {
    let bytes = encode(&every_body()).unwrap();
    for end in 0..bytes.len() {
        assert_eq!(refused(&bytes[..end]), SqlState::CONNECTION_FAILURE, "cut at {end} of {}", bytes.len());
    }
}

/// Verifies: [DESIGN] RULES.md D275, D172: a form of another format, a
/// node of no operator, a node only kept but made, a join of no kind, a
/// set operator, expression or operator of no code is refused, SQLSTATE
/// 38T03.
#[test]
fn a_code_no_operator_has_is_refused() {
    let mut other = head(Op::Distinct.code());
    other[0] = 2;
    assert_eq!(refused(&other), VIOLATION, "format 2");
    assert_eq!(refused(&head(0)), VIOLATION, "operator 0");
    assert_eq!(refused(&head(99)), VIOLATION, "operator 99");
    for op in Op::ALL.into_iter().filter(|o| !o.made()) {
        assert_eq!(refused(&head(op.code())), VIOLATION, "{op:?} made");
    }
    let mut join = head(Op::Join.code());
    join.put_u8(4);
    assert_eq!(refused(&join), VIOLATION, "join kind 4");
    for code in [0_u8, 4] {
        let mut set = head(Op::Set.code());
        set.extend_from_slice(&[code, 0]);
        assert_eq!(refused(&set), VIOLATION, "set operator {code}");
    }
    let mut expr = head(Op::Filter.code());
    expr.put_u8(7);
    assert_eq!(refused(&expr), VIOLATION, "expression 7");
    let mut operator = head(Op::Filter.code());
    operator.extend_from_slice(&[4, 17]);
    assert_eq!(refused(&operator), VIOLATION, "operator 17");
}

/// The stack of the thread a test of the deepest forms runs on: a test
/// build's frames of a form read are larger than a release's.
const DEEP_STACK: usize = 16 * 1024 * 1024;

/// A form of `n` nodes, each the input of the next.
fn nodes(n: usize) -> Vec<u8> {
    let mut root = made(Op::Distinct, Body::Distinct, Vec::new());
    for _ in 1..n {
        root = made(Op::Distinct, Body::Distinct, vec![root]);
    }
    encode(&Form { root }).unwrap()
}

/// A form of a filter of `n` expressions, each the operand of the next.
fn exprs(n: usize) -> Vec<u8> {
    let mut e = Expression::Column(0);
    for _ in 1..n {
        e = Expression::Cast { data_type: "INTEGER".into(), arg: Box::new(e) };
    }
    encode(&Form { root: made(Op::Filter, Body::Filter(e), Vec::new()) }).unwrap()
}

/// Verifies: [DESIGN] RULES.md D275, R-CF-6: a form whose nodes, or
/// whose expressions, nest to `MAX_DEPTH` is read, and one nesting past
/// it is refused, SQLSTATE 38T03.
#[test]
fn a_form_nested_past_the_limit_is_refused() {
    let run = || {
        assert!(read(&nodes(MAX_DEPTH)).is_ok());
        assert_eq!(refused(&nodes(MAX_DEPTH + 1)), VIOLATION);
        assert!(read(&exprs(MAX_DEPTH - 1)).is_ok());
        assert_eq!(refused(&exprs(MAX_DEPTH)), VIOLATION);
    };
    std::thread::Builder::new().name("form-depth".to_owned()).stack_size(DEEP_STACK).spawn(run).unwrap().join().unwrap();
}

/// Verifies: [DESIGN] RULES.md D275, D172: a value protocol 1 does not
/// carry, a reference, is refused as the form is written, in a literal
/// and in a column's bounds alike, SQLSTATE 38T03.
#[test]
fn a_value_no_protocol_carries_is_not_written() {
    let reference = Value::Reference(Binary::new(vec![1, 2]));
    let literal = Expression::Literal { data_type: String::new(), value: reference.clone() };
    let in_literal = Form { root: made(Op::Project, Body::Project(vec![literal]), Vec::new()) };
    assert_eq!(encode(&in_literal).unwrap_err().sqlstate(), VIOLATION);
    let low = ColumnStatistics { distinct: 1.0, nulls: 0.0, bounds: Some((reference.clone(), int_value(1))) };
    let high = ColumnStatistics { distinct: 1.0, nulls: 0.0, bounds: Some((int_value(1), reference)) };
    for s in [low, high] {
        let scan = Form { root: made(Op::Scan, Body::Scan { table: "U.T".into(), statistics: vec![s] }, Vec::new()) };
        assert_eq!(encode(&scan).unwrap_err().sqlstate(), VIOLATION);
    }
}
