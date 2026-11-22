//! Unit tests: when a correlated subquery whose run is one probe of a
//! unique key is decorrelated (RULES.md D790, R-TEST-1, R-TEST-8).

use std::sync::Arc;

use super::super::{Decorrelation, Subqueries};
use super::*;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::logical::{Cond, Expr, LogicalPlan, Subquery};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, Decimal, Value};

/// A state whose table `A.U` (K INTEGER, W INTEGER) holds 200 rows, W = K
/// for each: K the primary key if `unique`, else indexed by `UK` and of
/// 10 values; its subqueries decorrelated after 8 runs.
fn state(unique: bool) -> Arc<State> {
    let db = crate::db::Database::new();
    db.set_setting("EXECUTOR.DECORRELATE_AFTER_RUNS", &int(8)).unwrap();
    let key = if unique { ", PRIMARY KEY (K)" } else { "" };
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE U (K INTEGER NOT NULL, W INTEGER{key})")).unwrap();
    let s = db.session("A");
    let keys: i128 = if unique { 200 } else { 10 };
    let rows: Vec<String> = (0..200_i128).map(|n| format!("({}, {})", n.rem_euclid(keys), n.rem_euclid(keys))).collect();
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

/// The runs of `sq` by `runner`, one for each value from 0, until it is
/// decorrelated or has run `most` times: how many it ran, and whether it
/// was decorrelated. Each run gives its value's row, W = K.
fn runs_until_built(runner: &Runner, sq: &Subquery, most: i128) -> (i128, bool) {
    for n in 0..most {
        let rows = runner.rows(sq, &[int(n)], None).unwrap();
        let first = rows.rows().unwrap().next().map(|r| r.unwrap());
        assert_eq!(format!("{first:?}"), format!("{:?}", Some(vec![int(n)])), "run {n}");
        if matches!(runner.seen.borrow().get(&sq.id()).map(|s| &s.decorrelation), Some(Decorrelation::Keyed(_))) {
            return (n.saturating_add(1), true);
        }
    }
    (most, false)
}

/// Verifies: [DESIGN] RULES.md D790: a subquery whose run is one probe of
/// a whole unique key is probed past EXECUTOR.DECORRELATE_AFTER_RUNS, and
/// decorrelated only once its probes, `PROBE_ROWS` rows each, reach the
/// rows its build would hold: of 200 rows, at the first run whose probes
/// reach them, each run till then giving its row. MC/DC pairs of staying probed: a unique key's
/// probe, kept; one of a key that is not unique, decorrelated after 8
/// runs, as before.
#[test]
fn a_unique_probe_is_decorrelated_once_its_probes_cost_a_build() {
    let unique = state(true);
    let (runs, built) = runs_until_built(&Runner::new(&unique, NO_TRANSACTION), &subquery(), 100);
    assert!(built, "decorrelated in the end");
    // The first run whose probes, PROBE_ROWS rows each, reach the 200 rows.
    let first = (1..=200_i128).find(|r| f64::from(i32::try_from(*r).unwrap()) * PROBE_ROWS >= 200.0_f64).unwrap();
    assert_eq!(runs, first, "200 rows over {PROBE_ROWS} a probe");
    let shared = state(false);
    let (runner, sq) = (Runner::new(&shared, NO_TRANSACTION), subquery());
    for n in 0..9 {
        let rows = runner.rows(&sq, &[int(n)], None).unwrap();
        assert_eq!(rows.rows().unwrap().count(), 20, "run {n}");
    }
    let seen = runner.seen.borrow();
    assert!(matches!(seen.get(&sq.id()).map(|s| &s.decorrelation), Some(Decorrelation::Keyed(_))), "after 8 runs");
}
