//! Executor fallback and lazy-error boundaries below SQL cost selection.

use super::*;
use crate::catalog::QualifiedName;
use crate::db::Database;
use crate::functions::ScalarRef;
use crate::geometry::{Geometry, GeometryFunction};
use crate::logical::Expr;
use crate::txn::NO_TRANSACTION;

fn state() -> Arc<State> {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, G GEOMETRY(0))").unwrap();
    db.create_index("T_G", "A.T", &["G"], "rtree").unwrap();
    db.snapshot().unwrap()
}

fn read(value: Value, first: usize) -> NearestRead {
    NearestRead {
        table: QualifiedName { auth: "A".into(), ident: "T".into() },
        index: "T_G".into(),
        position: 0,
        method: "rtree".into(),
        columns: vec![0, 1],
        located: false,
        width: 2,
        measure: Expr::Call(ScalarRef(Arc::new(GeometryFunction::Distance)), vec![Expr::Column(1), Expr::Const(value)]),
        descending: false,
        condition: None,
        first,
        detail: String::new(),
        cost: 0.0,
        approximate: None,
        scale: None,
    }
}

/// Verifies: [DESIGN] D15, D38, D85: requests that cannot be certified
/// do not manufacture a distance error on no rows. An unaffordable
/// threshold falls back before opening the candidate stream.
#[test]
fn nearest_geometry_fallbacks_preserve_empty_input_and_memory() {
    let state = state();
    let before = state.memory.reserved();
    for value in [
        Value::Null,
        Value::Char("invalid geometry".into()),
        Geometry::from_wkt("POINT EMPTY", 4326).unwrap().to_value(),
        Geometry::point(f64::MAX, 0.0, 0).unwrap().to_value(),
    ] {
        let found = open(read(value, 1), &state, NO_TRANSACTION).unwrap().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(found.len(), 0);
        assert_eq!(state.memory.reserved(), before);
    }
    let point = Geometry::point(0.0, 0.0, 0).unwrap().to_value();
    let found = open(read(point, usize::MAX), &state, NO_TRANSACTION).unwrap().collect::<Result<Vec<_>>>().unwrap();
    assert_eq!(found.len(), 0);
    assert_eq!(state.memory.reserved(), before);
}

/// Verifies: [DESIGN] R-ERR-1: a malformed physical nearest node reports
/// SQLCODE -910 instead of panicking, including an absent table/index,
/// an unfilled scalar operand and dispatch with the wrong operator.
#[test]
fn nearest_geometry_broken_plans_report_api_errors() {
    let state = state();
    let query = read(Geometry::point(0.0, 0.0, 0).unwrap().to_value(), 1);
    let mut missing = query.clone();
    missing.table.ident = "MISSING".into();
    assert_eq!(open(missing, &state, NO_TRANSACTION).err().unwrap().code, crate::error::Code::Api);
    let mut missing_index = query.clone();
    missing_index.index = "MISSING".into();
    assert_eq!(open(missing_index, &state, NO_TRANSACTION).err().unwrap().code, crate::error::Code::Api);
    let mut unfilled = query;
    unfilled.measure = Expr::Column(0);
    assert_eq!(open(unfilled, &state, NO_TRANSACTION).err().unwrap().code, crate::error::Code::Api);
    let wrong = Plan::Values { rows: Vec::new(), detail: String::new() };
    assert_eq!(nearest(wrong, &state, NO_TRANSACTION).next().unwrap().unwrap_err().code, crate::error::Code::Api);
}
