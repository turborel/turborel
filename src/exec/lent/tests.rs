//! Unit tests: which kept plans are read where they are kept, and that
//! each gives what the executor gives of a copy (RULES.md D903, R-TEST-1,
//! R-TEST-8).

use std::sync::Arc;

use super::super::{Slots, execute};
use super::*;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::logical::{Cond, LogicalPlan, Slot, Subquery};
use crate::txn::NO_TRANSACTION;
use crate::value::{ArithOp, CompOp, DataType, Decimal, Value};

/// A state whose table `A.U` (K INTEGER, W INTEGER) holds 50 rows, K its
/// primary key and W a thousand more.
fn state() -> Arc<State> {
    let db = crate::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE U (K INTEGER NOT NULL PRIMARY KEY, W INTEGER)").unwrap();
    let s = db.session("A");
    let rows: Vec<String> = (0..50_i128).map(|n| format!("({n}, {})", n.saturating_add(1000))).collect();
    s.run(&format!("INSERT INTO U VALUES {}", rows.join(", "))).unwrap();
    s.commit().unwrap();
    db.snapshot().unwrap()
}

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn array(of: &[i128]) -> Value {
    Value::Array(of.iter().map(|&n| int(n)).collect())
}

/// One row of no column.
fn unit() -> Plan {
    Plan::Values { rows: vec![Vec::new()], detail: String::new() }
}

/// A collection derived table of `collections` over `input`, WITH
/// ORDINALITY.
fn unnested(collections: Vec<Expr>, input: Plan) -> Plan {
    let spread = collections.iter().map(|_| None).collect();
    let u = Unnest { collections, spread, ordinality: true, outer: None, names: Vec::new(), input, json: None, graph: None };
    Plan::Unnest(Box::new(u))
}

fn projected(exprs: Vec<Expr>, input: Plan) -> Plan {
    Plan::Project { exprs, detail: String::new(), input: Box::new(input) }
}

/// The slot `index`, of an exact number.
fn slot(index: usize) -> Expr {
    let data_type = super::super::slot_type(&int(0)).unwrap_or(DataType::Boolean);
    Expr::Dynamic(Slot { index, data_type, at: 0 })
}

fn arith(left: Expr, op: ArithOp, right: Expr) -> Expr {
    Expr::Arith(Box::new(left), op, Box::new(right))
}

/// `(SELECT W FROM U WHERE K = ?)`, its `?` the column `outer` of the row
/// it is applied to.
fn subquery(outer: usize) -> Expr {
    let table = QualifiedName { auth: "A".into(), ident: "U".into() };
    let scan = LogicalPlan::Scan { table, names: vec!["K".into(), "W".into()], needed: vec![0, 1], located: false };
    let cond = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Outer(0, "X.I".into()));
    let filter = LogicalPlan::Filter { cond, input: Box::new(scan) };
    let plan = LogicalPlan::Project { exprs: vec![Expr::Column(1)], input: Box::new(filter) };
    Expr::Subquery(Arc::new(Subquery::new(plan, vec![Expr::Column(outer)])), 0)
}

/// What `plan` gives in `db`, read where it is and by the executor of a
/// copy: each its rows or its error, as text.
fn both(plan: &Plan, db: &Db) -> (String, String) {
    let lent = Lent::of(plan, db).expect("read where it is kept");
    let mut rows = Vec::new();
    let read = lent.rows(db, NO_TRANSACTION, &mut |row| {
        rows.push(row);
        Ok(())
    });
    let by_executor: Result<Vec<Row>> = execute(plan.clone(), db, NO_TRANSACTION).collect();
    (format!("{:?}", read.map(|()| rows)), format!("{by_executor:?}"))
}

/// Verifies: [DESIGN] RULES.md D903: a projection of a collection derived
/// table over one row of no column, read where it is kept, gives the rows
/// the executor gives of a copy: of arrays of unequal cardinality, the
/// shorter's elements null past its last, with the ordinal; of a null
/// collection, none; of a value its slot holds; of a
/// scalar subquery for each row, of a key found, found again and
/// missing; and of an expression that raises at its second row, that
/// error and no row.
#[test]
fn a_projection_of_an_unnest_is_read_as_the_executor_reads_it() {
    let st = state();
    let slots: Slots = Arc::from(vec![Value::Null, int(7)]);
    let db = Db::slotted(&st, slots);
    let sum = arith(Expr::Column(0), ArithOp::Add, Expr::Column(2));
    let unequal = projected(vec![sum, Expr::Column(1)], unnested(vec![Expr::Const(array(&[1, 2, 3])), Expr::Const(array(&[5]))], unit()));
    let (read, run) = both(&unequal, &db);
    assert_eq!(read, run);
    assert_eq!(read, format!("{:?}", Ok::<_, ()>(vec![vec![int(2), int(5)], vec![int(4), Value::Null], vec![int(6), Value::Null]])));
    let null = projected(vec![arith(Expr::Column(0), ArithOp::Add, slot(1))], unnested(vec![Expr::Const(Value::Null)], unit()));
    assert_eq!(both(&null, &db), ("Ok([])".to_owned(), "Ok([])".to_owned()));
    let slotted = projected(
        vec![arith(Expr::Column(0), ArithOp::Mul, slot(1)), subquery(0)],
        unnested(vec![Expr::Const(array(&[3, 4, 3, 99]))], unit()),
    );
    let (of_slots, by_plan) = both(&slotted, &db);
    assert_eq!(of_slots, by_plan);
    let expected = vec![vec![int(21), int(1003)], vec![int(28), int(1004)], vec![int(21), int(1003)], vec![int(693), Value::Null]];
    assert_eq!(of_slots, format!("{:?}", Ok::<_, ()>(expected)));
    let divided = arith(Expr::Const(int(10)), ArithOp::Div, Expr::Column(0));
    let dividing = projected(vec![divided, Expr::Column(1)], unnested(vec![Expr::Const(array(&[5, 0, 2]))], unit()));
    let (raised, by_plan_raised) = both(&dividing, &db);
    assert_eq!(raised, by_plan_raised);
    assert!(raised.starts_with("Err("), "{raised}");
}

/// Verifies: [DESIGN] RULES.md D903: any other plan is opened by the
/// executor. MC/DC pairs of reading a plan where it is kept: a
/// projection of an expression over an unnest of one row of no column,
/// read so; of its columns as they are, not; over a row of a column, or
/// over two rows, not; an unnest alone, a projection of values, not.
#[test]
fn any_other_plan_is_opened_by_the_executor() {
    let db = Db::of(&state());
    let of_array = || vec![Expr::Const(array(&[1, 2]))];
    let sum = || arith(Expr::Column(0), ArithOp::Add, Expr::Column(1));
    assert!(Lent::of(&projected(vec![sum()], unnested(of_array(), unit())), &db).is_some());
    assert!(Lent::of(&projected(vec![Expr::Column(1), Expr::Column(0)], unnested(of_array(), unit())), &db).is_none(), "columns alone");
    let one_column = Plan::Values { rows: vec![vec![Expr::Const(int(1))]], detail: String::new() };
    assert!(Lent::of(&projected(vec![sum()], unnested(of_array(), one_column)), &db).is_none(), "over a row of a column");
    let two_rows = Plan::Values { rows: vec![Vec::new(), Vec::new()], detail: String::new() };
    assert!(Lent::of(&projected(vec![sum()], unnested(of_array(), two_rows)), &db).is_none(), "over two rows");
    assert!(Lent::of(&unnested(of_array(), unit()), &db).is_none(), "no projection");
    assert!(Lent::of(&projected(vec![Expr::Const(int(1))], unit()), &db).is_none(), "a projection of values");
}
