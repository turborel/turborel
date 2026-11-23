//! Unit tests: which runs of a correlated subquery are read by the point
//! read its plan kept is, and what each gives (RULES.md D900, R-TEST-1,
//! R-TEST-8).

use std::sync::Arc;

use super::super::{Db, KeptSubqueries, Subqueries};
use super::*;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::logical::{Cond, Expr, LogicalPlan};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, Decimal};

/// A state whose table `A.U` (K INTEGER, W INTEGER) holds 200 rows, W a
/// thousand more than K for each: K the primary key if `unique`, else
/// indexed by `UK` and of 10 values; with the settings `set`.
fn state(unique: bool, set: &[(&str, Value)]) -> Arc<State> {
    let db = crate::db::Database::new();
    for (name, value) in set {
        db.set_setting(name, value).unwrap();
    }
    let key = if unique { ", PRIMARY KEY (K)" } else { "" };
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE U (K INTEGER NOT NULL, W INTEGER{key})")).unwrap();
    let s = db.session("A");
    let keys: i128 = if unique { 200 } else { 10 };
    let rows: Vec<String> =
        (0..200_i128).map(|n| format!("({}, {})", n.rem_euclid(keys), n.rem_euclid(keys).saturating_add(1000))).collect();
    s.run(&format!("INSERT INTO U VALUES {}", rows.join(", "))).unwrap();
    s.commit().unwrap();
    if !unique {
        s.run("CREATE INDEX UK ON U (K)").unwrap();
        s.commit().unwrap();
    }
    db.snapshot().unwrap()
}

/// `SELECT W FROM U WHERE K = ?`, its `?` the outer reference `T.K`.
fn subquery() -> Subquery {
    let table = QualifiedName { auth: "A".into(), ident: "U".into() };
    let scan = LogicalPlan::Scan { table, names: vec!["K".into(), "W".into()], needed: vec![0, 1], located: false };
    let cond = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Outer(0, "T.K".into()));
    let filter = LogicalPlan::Filter { cond, input: Box::new(scan) };
    Subquery::new(LogicalPlan::Project { exprs: vec![Expr::Column(1)], input: Box::new(filter) }, vec![Expr::Column(0)])
}

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The rows `rows` holds, as text.
fn shown(rows: &SubqueryRows) -> String {
    format!("{:?}", rows.rows().unwrap().map(|r| r.unwrap()).collect::<Vec<_>>())
}

/// What the point read of `sq` gives `runner` for `outer`, as text; none
/// if the run is not read so.
fn pointed(runner: &Runner, sq: &Subquery, outer: &[Value]) -> Option<String> {
    runner.pointed(sq, outer, None).unwrap().map(|rows| shown(&rows))
}

/// Verifies: [DESIGN] RULES.md D900: a subquery whose plan is one probe of
/// a whole unique key is read by its point read from its second run, its
/// first having kept the plan, and gives the row its plan gives: the row
/// of its key, none of a key no row has, and of a limit of none, none. A
/// decimal of another scale than its slot's is placed as it is. MC/DC
/// pairs of reading so: a plan kept, read so, and none kept yet, not; a
/// value of its slot's kind, read so, and a null or a string, not.
#[test]
fn a_kept_unique_probe_is_read_as_a_point_read() {
    let st = state(true, &[]);
    let (runner, sq) = (Runner::new(&st, NO_TRANSACTION), subquery());
    assert_eq!(pointed(&runner, &sq, &[int(3)]), None, "no plan kept before its first run");
    assert_eq!(shown(&runner.rows(&sq, &[int(3)], None).unwrap()), format!("{:?}", vec![vec![int(1003)]]));
    assert_eq!(pointed(&runner, &sq, &[int(7)]), Some(format!("{:?}", vec![vec![int(1007)]])));
    assert_eq!(pointed(&runner, &sq, &[int(999)]), Some("[]".to_owned()), "a key no row has");
    assert_eq!(pointed(&runner, &sq, &[Value::Exact(Decimal::new(70, 1))]), Some(format!("{:?}", vec![vec![int(1007)]])), "7.0, as it is");
    assert_eq!(runner.pointed(&sq, &[int(7)], Some(0)).unwrap().map(|rows| shown(&rows)), Some("[]".to_owned()));
    assert_eq!(pointed(&runner, &sq, &[Value::Null]), None, "a null is planned alone");
    assert_eq!(pointed(&runner, &sq, &[Value::Char("7".into())]), None, "another kind is planned alone");
    assert_eq!(pointed(&runner, &sq, &[]), None, "no outer value");
    // Each run through the runner gives what the plan gives, and is kept.
    for n in [7_i128, 8, 7, 999] {
        let expected = if n < 200 { vec![vec![int(n.saturating_add(1000))]] } else { Vec::new() };
        assert_eq!(shown(&runner.rows(&sq, &[int(n)], None).unwrap()), format!("{expected:?}"), "run of {n}");
    }
    assert_eq!(shown(&runner.rows(&sq, &[Value::Null], None).unwrap()), "[]");
}

/// Verifies: [DESIGN] RULES.md D900: a plan that is no point read, a probe
/// of a key that is not unique, kept once its runs agree, is not read so;
/// nor is a unique key's with EXECUTOR.POINT_OPERATIONS OFF or with
/// EXECUTOR.KEEP_SUBQUERY_PLANS OFF; each run gives its rows by its plan.
#[test]
fn a_plan_that_is_no_point_read_is_run_by_the_executor() {
    let many = [("EXECUTOR.DECORRELATE_AFTER_RUNS", int(1_000_000))];
    let shared = state(false, &many);
    let (runner, sq) = (Runner::new(&shared, NO_TRANSACTION), subquery());
    for n in 0..20_i128 {
        assert_eq!(runner.rows(&sq, &[int(n.rem_euclid(10))], None).unwrap().rows().unwrap().count(), 20, "run {n}");
        assert_eq!(pointed(&runner, &sq, &[int(1)]), None, "after run {n}");
    }
    for off in ["EXECUTOR.POINT_OPERATIONS", "EXECUTOR.KEEP_SUBQUERY_PLANS"] {
        let st = state(true, &[(off, Value::Char("OFF".into()))]);
        let unread = Runner::new(&st, NO_TRANSACTION);
        assert_eq!(shown(&unread.rows(&sq, &[int(3)], None).unwrap()), format!("{:?}", vec![vec![int(1003)]]));
        assert_eq!(pointed(&unread, &sq, &[int(7)]), None, "{off} OFF");
        assert_eq!(shown(&unread.rows(&sq, &[int(7)], None).unwrap()), format!("{:?}", vec![vec![int(1007)]]));
    }
}

/// Verifies: [DESIGN] RULES.md D900, D793: the runner of a later execution
/// of a kept plan reads a subquery an earlier one settled by its point
/// read from its first run, the plan kept shared with it, not copied.
#[test]
fn a_later_execution_reads_by_the_point_read_an_earlier_one_kept() {
    let st = state(true, &[]);
    let (kept, sq) = (Arc::new(KeptSubqueries::default()), subquery());
    let first = Runner::new(&Db::of(&st).keeping(Arc::clone(&kept)), NO_TRANSACTION);
    assert_eq!(shown(&first.rows(&sq, &[int(3)], None).unwrap()), format!("{:?}", vec![vec![int(1003)]]));
    let later = Runner::new(&Db::of(&st).keeping(Arc::clone(&kept)), NO_TRANSACTION);
    assert_eq!(pointed(&later, &sq, &[int(9)]), Some(format!("{:?}", vec![vec![int(1009)]])));
    let (Some(Planned::Kept(a)), Some(Planned::Kept(b))) = (kept.get(sq.id()), kept.get(sq.id())) else { panic!("kept") };
    assert!(Arc::ptr_eq(&a, &b), "shared");
    let alone = Runner::new(&st, NO_TRANSACTION);
    assert_eq!(pointed(&alone, &sq, &[int(9)]), None, "a runner of no kept plan settles its own");
}
