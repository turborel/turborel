//! Unit tests of point reads. A plan of one probe of a whole unique key,
//! alone or under a projection of its columns, is one and reads the row
//! the plan gives, and any other plan is none (RULES.md R-TEST-1).

use std::sync::Arc;

use super::PointRead;
use crate::db::{Database, State};
use crate::engine::rows::{COLUMN_STORE, ROW_STORE};
use crate::error::Result;
use crate::exec::execute;
use crate::logical::{Cond, Expr, LogicalPlan};
use crate::physical::{PhysicalPlanner, Plan};
use crate::storage::Row;
use crate::txn::NO_TRANSACTION;
use crate::value::{ArithOp, CompOp, Decimal, Value};

/// A database whose tables `engine` keeps, of `S.T` (K, N, V), K its
/// primary key, filled with each K from 0 to 599, N its tenth and V ten
/// times it, N with an index of its own.
fn filled(engine: &str) -> (Database, Arc<State>) {
    let db = Database::new();
    db.use_engine(engine).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER, V INTEGER)").unwrap();
    let s = db.session("S");
    let values: Vec<String> = (0_i32..600_i32).map(|k| format!("({k}, {}, {k}0)", k.div_euclid(10_i32))).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    db.create_index("T_N", "S.T", &["N"], crate::index::BTREE_METHOD).unwrap();
    let st = db.snapshot().unwrap();
    (db, st)
}

fn int(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// The rows of `T` whose column `column` is `n`, of the columns `exprs`
/// if given.
fn probe(column: usize, n: i128, exprs: Option<Vec<Expr>>) -> LogicalPlan {
    let table = crate::catalog::QualifiedName { auth: "S".into(), ident: "T".into() };
    let names = vec!["K".to_owned(), "N".to_owned(), "V".to_owned()];
    let scan = LogicalPlan::Scan { table, names, needed: vec![0, 1, 2], located: false };
    let cond = Cond::Compare(Expr::Column(column), CompOp::Eq, int(n));
    let filtered = LogicalPlan::Filter { cond, input: Box::new(scan) };
    match exprs {
        Some(exprs) => LogicalPlan::Project { exprs, input: Box::new(filtered) },
        None => filtered,
    }
}

/// The plan of `logical`, its rows as the executor runs it, and its point
/// read, if it is one.
fn planned(st: &Arc<State>, logical: &LogicalPlan) -> (Vec<Row>, Option<PointRead>) {
    let plan: Plan = PhysicalPlanner::new(st).plan(logical);
    let point = PointRead::of(&plan, st);
    (execute(plan, st, NO_TRANSACTION).collect::<Result<Vec<Row>>>().unwrap(), point)
}

fn shown(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>().join("|")).collect()
}

/// Verifies: [DESIGN] RULES.md D752: a probe of the whole primary key,
/// alone, or projected to its columns in another order or one of them
/// twice, is a point read, which reads the rows the plan gives, of either
/// engine.
#[test]
fn a_probe_of_a_whole_unique_key_is_read_as_its_plan_reads() {
    for engine in [ROW_STORE, COLUMN_STORE] {
        let (_db, st) = filled(engine);
        let shapes = [None, Some(vec![Expr::Column(2), Expr::Column(0)]), Some(vec![Expr::Column(2), Expr::Column(2), Expr::Column(1)])];
        for exprs in shapes {
            for key in [7, 599] {
                let (rows, point) = planned(&st, &probe(0, key, exprs.clone()));
                let point = point.unwrap_or_else(|| panic!("{engine}: {exprs:?} is a point read"));
                let read: Vec<Row> = point.read(&st, NO_TRANSACTION, &[]).unwrap().into_iter().collect();
                assert_eq!(shown(&read), shown(&rows), "{engine}: {exprs:?} of {key}");
            }
        }
        let (rows, point) = planned(&st, &probe(0, 7, Some(vec![Expr::Column(2), Expr::Column(0)])));
        assert_eq!(shown(&rows), ["70|7"], "{engine}");
        assert!(point.is_some(), "{engine}");
    }
}

/// Verifies: [DESIGN] RULES.md D752: a probe of an index not unique, or
/// a projection computing a value, is no point read.
#[test]
fn a_probe_of_many_rows_or_a_computed_value_is_no_point_read() {
    let (_db, st) = filled(ROW_STORE);
    let (rows, point) = planned(&st, &probe(1, 3, None));
    assert_eq!(rows.len(), 10);
    assert!(point.is_none(), "a key of ten rows");
    let doubled = Expr::Arith(Box::new(Expr::Column(2)), ArithOp::Add, Box::new(int(1)));
    let (_, computed) = planned(&st, &probe(0, 7, Some(vec![doubled])));
    assert!(computed.is_none(), "a value computed");
}

/// Verifies: [DESIGN] RULES.md D752: a plan kept is run as its point read
/// while EXECUTOR.POINT_OPERATIONS is on, and through the executor once
/// it is off.
#[test]
fn point_operations_off_run_a_kept_plan_through_the_executor() {
    let (db, st) = filled(ROW_STORE);
    let plan = PhysicalPlanner::new(&st).plan(&probe(0, 7, None));
    let kept = crate::physical::prepared_plan(plan.clone(), None).unwrap();
    assert!(kept.point(&st).is_some());
    db.set_setting("EXECUTOR.POINT_OPERATIONS", &Value::Char("OFF".into())).unwrap();
    let off = db.snapshot().unwrap();
    let off_kept = crate::physical::prepared_plan(plan, None).unwrap();
    assert!(off_kept.point(&off).is_none());
}
