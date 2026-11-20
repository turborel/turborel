//! Unit tests: a plan taken back from a form a rule gave, each node the
//! form makes accepted, and each form not valid refused, SQLSTATE 38T03
//! (RULES.md R-TEST-1, R-TEST-2). The fixtures here serve `exprs.rs`'s
//! tests too.

use std::sync::Arc;

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::logical::{Expr, Limit, LogicalPlan};
use crate::plugin::errors::VIOLATION;
use crate::plugin::plan::{Body, Column, Expression, Form, JoinKind, Kept, Key, MAX_DEPTH, Node, Op, SetKind, form_of, plan_of};
use crate::value::{Decimal, Value};

/// A database's state, its catalog a table U.T (A INTEGER, B INTEGER,
/// C CHARACTER(3)).
pub(in crate::plugin::plan) fn state() -> Arc<State> {
    let db = crate::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (A INTEGER, B INTEGER, C CHARACTER(3))").unwrap();
    db.snapshot().unwrap()
}

/// A scan of U.T, every column needed.
pub(in crate::plugin::plan) fn scan() -> LogicalPlan {
    LogicalPlan::Scan {
        table: QualifiedName { auth: "U".into(), ident: "T".into() },
        names: vec!["A".into(), "B".into(), "C".into()],
        needed: vec![0, 1, 2],
        located: false,
    }
}

/// A node of `op`, made, of `body` over `inputs`.
pub(in crate::plugin::plan) fn made(op: Op, body: Body, inputs: Vec<Node>) -> Node {
    Node { op, id: 0, kept: false, columns: Vec::new(), rows: -1.0, width: -1.0, body, inputs }
}

/// A plan's form, what it keeps, and the state it was made in.
pub(in crate::plugin::plan) struct Given {
    pub(in crate::plugin::plan) st: Arc<State>,
    pub(in crate::plugin::plan) form: Form,
    pub(in crate::plugin::plan) kept: Kept,
}

impl Given {
    /// The form of `plan`.
    pub(in crate::plugin::plan) fn of(plan: &LogicalPlan) -> Given {
        let st = state();
        let (form, kept) = form_of(plan, &st).unwrap();
        Given { st, form, kept }
    }

    /// The plan of `root`, given back for the plan this form is of.
    pub(in crate::plugin::plan) fn back(&self, root: Node) -> Result<LogicalPlan> {
        plan_of(&Form { root }, &self.kept, &self.st)
    }
}

/// That `r` is refused as a plan not valid, SQLSTATE 38T03.
pub(in crate::plugin::plan) fn refused(r: Result<LogicalPlan>, what: &str) {
    match r {
        Ok(p) => panic!("{what} was taken back: {}", p.explain()),
        Err(e) => assert_eq!(e.sqlstate(), VIOLATION, "{what}: {}", e.msg),
    }
}

/// A projection of a node's first three columns, as U.T's are.
fn three(input: Node) -> Node {
    let columns = (0..3).map(Expression::Column).collect();
    made(Op::Project, Body::Project(columns), vec![input])
}

/// A node of VALUES of U.T's column types, of `rows`.
fn values(rows: Vec<Vec<Expression>>) -> Node {
    let typed = |t: &str| Column { name: "V".into(), data_type: t.into() };
    let columns = vec![typed("INTEGER"), typed("INTEGER"), typed("CHARACTER(3)")];
    Node { columns, ..made(Op::Values, Body::Values(rows), Vec::new()) }
}

/// A row of VALUES of U.T's column types.
fn row(n: i128) -> Vec<Expression> {
    let int = |data_type: &str| Expression::Literal { data_type: data_type.into(), value: Value::Exact(Decimal::new(n, 0)) };
    vec![int("INTEGER"), int("INTEGER"), Expression::Literal { data_type: "CHARACTER(3)".into(), value: Value::Char("abc".into()) }]
}

/// Verifies: [DESIGN] RULES.md D275, D277: a form a rule made of joins of
/// each kind, each set operator, VALUES, ORDER BY and DISTINCT, keeping
/// the plan's columns and types, is taken back as the planner's nodes of
/// each.
#[test]
fn each_node_a_rule_makes_is_taken_back() {
    let g = Given::of(&scan());
    let s = || g.form.root.clone();
    let cond = Expression::Operator { op: crate::plugin::plan::Operator::Eq, args: vec![Expression::Column(0), Expression::Column(4)] };
    let cross = made(Op::Join, Body::Join { kind: JoinKind::Cross, condition: None }, vec![s(), s()]);
    assert!(g.back(three(cross)).unwrap().explain().contains("Product"));
    for (kind, word) in [(JoinKind::Left, "LEFT"), (JoinKind::Right, "RIGHT"), (JoinKind::Full, "FULL")] {
        let join = made(Op::Join, Body::Join { kind, condition: Some(cond.clone()) }, vec![s(), s()]);
        let shown = g.back(three(join)).unwrap().explain();
        assert!(shown.to_uppercase().contains(word), "{shown}");
    }
    for (op, word) in [(SetKind::Union, "UNION"), (SetKind::Except, "EXCEPT"), (SetKind::Intersect, "INTERSECT")] {
        let shown = g.back(made(Op::Set, Body::Set { op, all: true }, vec![s(), s()])).unwrap().explain();
        assert!(shown.to_uppercase().contains(word), "{shown}");
    }
    assert!(g.back(values(vec![row(1), row(2)])).unwrap().explain().contains("Values"));
    let keys = vec![Key { column: 2, descending: true, nulls_first: true }];
    let sorted = made(Op::Distinct, Body::Distinct, vec![made(Op::Sort, Body::Sort(keys), vec![s()])]);
    let shown = g.back(sorted).unwrap().explain();
    assert!(shown.contains("Distinct") && shown.contains("Sort"), "{shown}");
}

/// Verifies: [DESIGN] RULES.md D277: a node of the wrong count of inputs,
/// a body of another operator, a scan of no table, a cross join with a
/// condition or an outer join without one, set operands of other degrees
/// or types not comparable, VALUES untyped or of rows of other degrees, a
/// sort key past the row, and an output of another type are each
/// refused.
#[test]
fn each_node_not_valid_is_refused() {
    let g = Given::of(&scan());
    let s = || g.form.root.clone();
    let scan_of = |table: &str| made(Op::Scan, Body::Scan { table: table.into(), statistics: Vec::new() }, Vec::new());
    refused(g.back(made(Op::Scan, Body::Scan { table: "U.T".into(), statistics: Vec::new() }, vec![s()])), "a scan of an input");
    refused(g.back(made(Op::Distinct, Body::Distinct, Vec::new())), "a distinct of none");
    refused(g.back(made(Op::Join, Body::Join { kind: JoinKind::Cross, condition: None }, vec![s()])), "a join of one");
    refused(g.back(made(Op::Set, Body::Set { op: SetKind::Union, all: false }, vec![s()])), "a set of one");
    refused(g.back(Node { inputs: vec![s()], ..values(vec![row(1)]) }), "VALUES of an input");
    refused(g.back(made(Op::Filter, Body::Distinct, vec![s()])), "a filter of another body");
    refused(g.back(made(Op::Sort, Body::Project(Vec::new()), vec![s()])), "a sort of another body");
    refused(g.back(scan_of("T")), "a table of no schema");
    refused(g.back(scan_of("U.NOWHERE")), "no such table");
    let cond = Some(Expression::Column(0));
    refused(g.back(three(made(Op::Join, Body::Join { kind: JoinKind::Cross, condition: cond }, vec![s(), s()]))), "cross on");
    refused(g.back(three(made(Op::Join, Body::Join { kind: JoinKind::Left, condition: None }, vec![s(), s()]))), "left");
    let one = made(Op::Project, Body::Project(vec![Expression::Column(0)]), vec![s()]);
    refused(g.back(made(Op::Set, Body::Set { op: SetKind::Union, all: false }, vec![s(), one])), "degrees");
    let turned = made(Op::Project, Body::Project([2, 1, 0].map(Expression::Column).to_vec()), vec![s()]);
    refused(g.back(made(Op::Set, Body::Set { op: SetKind::Except, all: false }, vec![s(), turned])), "types");
    refused(g.back(Node { columns: vec![Column { name: "V".into(), data_type: String::new() }], ..values(vec![]) }), "untyped");
    refused(g.back(values(vec![row(1), vec![]])), "a row of no values");
    let keys = vec![Key { column: 3, descending: false, nulls_first: false }];
    refused(g.back(made(Op::Sort, Body::Sort(keys), vec![s()])), "sort key 3 of 3");
    let other = made(Op::Project, Body::Project([0, 1, 0].map(Expression::Column).to_vec()), vec![s()]);
    refused(g.back(other), "a column of another type");
}

/// Verifies: [DESIGN] RULES.md D275, D277: a node the form keeps, a
/// limit, is taken back from the planner's with its input the rule gave,
/// of its input's types; one the form did not keep, one kept twice, one
/// of other inputs' count, or one whose input's types changed is refused.
#[test]
fn a_kept_node_is_taken_back_once() {
    let limit = Limit { offset: Some(Expr::Const(Value::Exact(Decimal::new(1, 0)))), fetch: None };
    let g = Given::of(&LogicalPlan::Limit { limit, input: Box::new(scan()) });
    let root = || g.form.root.clone();
    assert!(root().kept, "a limit is kept");
    let mut sorted = root();
    let keys = vec![Key { column: 0, descending: false, nulls_first: false }];
    sorted.inputs = vec![made(Op::Sort, Body::Sort(keys), vec![root().inputs[0].clone()])];
    let shown = g.back(sorted).unwrap().explain();
    assert!(shown.contains("Limit") && shown.contains("Sort"), "{shown}");
    refused(g.back(Node { id: 99, ..root() }), "a node not kept");
    refused(g.back(made(Op::Set, Body::Set { op: SetKind::Union, all: true }, vec![root(), root()])), "kept twice");
    refused(g.back(Node { inputs: Vec::new(), ..root() }), "a kept node of no input");
    let turned = made(Op::Project, Body::Project([2, 1, 0].map(Expression::Column).to_vec()), root().inputs);
    refused(g.back(Node { inputs: vec![turned], ..root() }), "an input of other types");
}

/// The stack of the thread a test of the deepest forms runs on: a test
/// build's frames of a plan taken back are larger than a release's.
const DEEP_STACK: usize = 16 * 1024 * 1024;

/// Verifies: [DESIGN] RULES.md D277, R-CF-6: a form of nodes nested to
/// `MAX_DEPTH` is taken back, and one nested past it is refused, never
/// followed.
#[test]
fn a_form_nested_past_the_limit_is_refused() {
    let deepest = |n: usize| {
        let g = Given::of(&scan());
        let mut deep = g.form.root.clone();
        for _ in 1..n {
            deep = made(Op::Distinct, Body::Distinct, vec![deep]);
        }
        g.back(deep)
    };
    let run = move || {
        assert!(deepest(MAX_DEPTH).is_ok());
        refused(deepest(MAX_DEPTH + 1), "nested past the limit");
    };
    std::thread::Builder::new().name("plan-depth".to_owned()).stack_size(DEEP_STACK).spawn(run).unwrap().join().unwrap();
}
