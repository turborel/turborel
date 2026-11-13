//! Unit tests of the physical planner: its response to a state that
//! breaks its invariants (RULES.md R-TEST-2 robustness), and the orders it
//! takes its plans to give.

use std::sync::Arc;

use super::*;
use crate::catalog::{Catalog, MemoryCatalog, TableDesc};
use crate::engine::rows::ROW_STORE;
use crate::error::{Code, Result};
use crate::exec::execute;
use crate::txn::NO_TRANSACTION;
use crate::value::{DataType, Spelled};

/// Verifies: [DESIGN] a catalog table without storage is planned as a
/// scan, which then reports SQLCODE -910 instead of the planner panicking.
#[test]
fn a_table_without_storage_is_scanned_and_reported() {
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema(
        "A",
        "A",
        vec![TableDesc {
            name: name.clone(),
            columns: vec![],
            constraints: vec![],
            engine: ROW_STORE.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        }],
    );
    let st = Arc::new(State::with_builtins(Box::new(catalog)));
    let conds = vec![Cond::Const(crate::logical::Truth::True)];
    let built = PhysicalPlanner::new(&st).access(TableRead { table: &name, shown: Shown::of(None), needed: &[] }, conds, &[]);
    assert!(matches!(&built.plan, Plan::Filter { input, .. } if matches!(**input, Plan::SeqScan { .. })));
    let failed = execute(built.plan, &st, NO_TRANSACTION).collect::<Result<Vec<_>>>();
    assert_eq!(failed.err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 GR2a]; [DESIGN]
/// RULES.md D35: groups of an index's rows in its order are in that order,
/// so ORDER BY their column needs no Sort; with a group made before any
/// row, as of an empty grouping set, they are not, so it does.
#[test]
fn groups_made_before_any_row_are_sorted() {
    let db = crate::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL, V INTEGER)").unwrap();
    let s = db.session("S");
    let values: Vec<String> = (0_i32..400_i32).map(|k| format!("({}, {k})", k % 20_i32)).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    db.create_index("T_K", "S.T", &["K"], crate::index::BTREE_METHOD).unwrap();
    db.with_cost_model(|c| c.sort_compare = 10.0_f64).unwrap();
    let st = db.snapshot().unwrap();
    let count = crate::functions::Registry::with_set_functions().aggregate("COUNT").unwrap();
    let sorted = |seeds: Vec<Row>| {
        let agg = AggCall {
            func: count.clone(),
            arg: None,
            distinct: false,
            result: DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric },
            label: "N".into(),
        };
        let scan = LogicalPlan::Scan {
            table: QualifiedName { auth: "S".into(), ident: "T".into() },
            names: vec!["K".into(), "V".into()],
            needed: vec![0],
        };
        let aggregate = LogicalPlan::Aggregate { groups: vec![0], collations: Vec::new(), aggs: vec![agg], seeds, input: Box::new(scan) };
        let logical = LogicalPlan::Sort { keys: vec![SortKey { column: 0, descending: false }], input: Box::new(aggregate) };
        matches!(PhysicalPlanner::new(&st).plan(&logical), Plan::Sort { .. })
    };
    assert!(!sorted(Vec::new()), "the index gives the groups' order");
    assert!(sorted(vec![vec![Value::Null]]), "a seeded group comes first");
}
