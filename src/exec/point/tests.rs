//! Unit tests of point reads. A plan of one probe of a whole unique key,
//! alone or under a projection of its columns, is one and reads the row
//! the plan gives, and any other plan is none (RULES.md R-TEST-1).

use std::borrow::Cow;
use std::sync::Arc;

use super::{PointRead, keyed};
use crate::db::{Database, State};
use crate::engine::rows::{COLUMN_STORE, ROW_STORE};
use crate::error::Result;
use crate::exec::execute;
use crate::index::Probe;
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

/// Verifies: [DESIGN] RULES.md D881: a point read of a primary key is
/// read through its index's probe of the whole key, of either engine,
/// its row the plan's and none of a key not there, and its search and
/// entry counted for the index; a value its column cannot place exactly
/// is not read so, and is searched, giving what the plan gives.
#[test]
fn a_point_read_probes_its_index_by_the_whole_key() {
    for engine in [ROW_STORE, COLUMN_STORE] {
        let (_db, st) = filled(engine);
        let (rows, point) = planned(&st, &probe(0, 7, None));
        let point = point.unwrap();
        let before = st.index_uses[&point.index].totals();
        let keyed::Probed::Read(found) = point.keyed(&st, NO_TRANSACTION, Cow::Borrowed(&point.probe), keyed::Asked::Whole).unwrap() else {
            panic!("{engine}: a whole key is probed")
        };
        assert_eq!(shown(&[found.unwrap().1]), shown(&rows), "{engine}");
        let after = st.index_uses[&point.index].totals();
        assert_eq!((after.0 - before.0, after.1 - before.1), (1, 1), "{engine}: one search of one entry");
        let (_, missing) = planned(&st, &probe(0, 5_000, None));
        let missing = missing.unwrap();
        assert!(
            matches!(
                missing.keyed(&st, NO_TRANSACTION, Cow::Borrowed(&missing.probe), keyed::Asked::Given).unwrap(),
                keyed::Probed::Read(None)
            ),
            "{engine}"
        );
        let fine = if matches!(point.probe, Probe::Prefix { .. }) {
            Probe::Prefix { equal: vec![Value::Exact(Decimal::new(75, 1))], then: None }
        } else {
            Probe::Compare { op: CompOp::Eq, value: Value::Exact(Decimal::new(75, 1)) }
        };
        assert!(
            matches!(point.keyed(&st, NO_TRANSACTION, Cow::Owned(fine), keyed::Asked::Whole).unwrap(), keyed::Probed::Unread(_)),
            "{engine}: 7.5 has no place"
        );
    }
}

/// Verifies: [DESIGN] RULES.md D917: an entry read for some columns is
/// the row of its table's width with its id, those columns in their
/// places, the key's among them; the row store decodes no other, which
/// are null. A list without the key's column is not read so: the row is
/// read whole, as with no list, and checked against its key all the same.
/// Of a key not there, none either way.
#[test]
fn an_entry_is_read_by_the_columns_asked_for() {
    let (_db, st) = filled(ROW_STORE);
    let (rows, point) = planned(&st, &probe(0, 7, None));
    let point = point.unwrap();
    let under = || Cow::Borrowed(&point.probe);
    let whole = point.entry_under(&st, NO_TRANSACTION, under()).unwrap().unwrap();
    assert_eq!(shown(std::slice::from_ref(&whole.1)), shown(&rows));
    let some = point.entry_reading(&st, NO_TRANSACTION, under(), Some(&[0, 2])).unwrap().unwrap();
    assert_eq!(some.0, whole.0, "the same row");
    assert_eq!(shown(&[some.1]), [format!("7|{}|70", Value::Null)], "N is not decoded");
    let keyless = point.entry_reading(&st, NO_TRANSACTION, under(), Some(&[2])).unwrap().unwrap();
    assert_eq!(shown(&[keyless.1]), shown(&rows), "read whole");
    let (_, missing) = planned(&st, &probe(0, 5_000, None));
    let missing = missing.unwrap();
    assert!(missing.entry_reading(&st, NO_TRANSACTION, Cow::Borrowed(&missing.probe), Some(&[0, 2])).unwrap().is_none());
}
