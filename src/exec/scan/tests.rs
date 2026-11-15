//! Unit tests of located reads: each row a scan, an index scan or an
//! index join reads of a table is followed by its id, of either engine,
//! and a batch is never made of them (RULES.md R-TEST-1, R-TEST-8).

use std::sync::Arc;

use crate::catalog::QualifiedName;
use crate::db::{Database, State};
use crate::engine::rows::{COLUMN_STORE, ROW_STORE};
use crate::error::Result;
use crate::exec::execute;
use crate::logical::{Cond, Expr, LogicalPlan, OuterJoin};
use crate::physical::{PhysicalPlanner, Plan};
use crate::storage::{ROW_ID, Row, row_id_of};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, DataType, Decimal, INTEGER_PRECISION, Spelled, Value};

/// The rows of `T`, each key K from 0 to `ROWS` - 1 with V ten times it.
const ROWS: i32 = 400;

/// The key a probe finds, and one no row has.
const FOUND: i128 = 7;
const MISSING: i128 = 1000;

/// A database whose tables `engine` keeps, of `S.T` (K, V) filled and an
/// index on K.
fn filled(engine: &str) -> Arc<State> {
    let db = Database::new();
    db.use_engine(engine).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL, V INTEGER)").unwrap();
    let s = db.session("S");
    let values: Vec<String> = (0_i32..ROWS).map(|k| format!("({k}, {k}0)")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    db.create_index("T_K", "S.T", &["K"], crate::index::BTREE_METHOD).unwrap();
    db.snapshot().unwrap()
}

fn table() -> QualifiedName {
    QualifiedName { auth: "S".into(), ident: "T".into() }
}

/// A scan of `T`, followed by each row's id if `located`.
fn scan(located: bool) -> LogicalPlan {
    let mut names = vec!["K".to_owned(), "V".to_owned()];
    if located {
        names.push(ROW_ID.to_owned());
    }
    LogicalPlan::Scan { table: table(), names, needed: vec![0, 1], located }
}

fn int(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// The rows of `logical`, as planned and run against `st`, and the plan.
fn run(st: &Arc<State>, logical: &LogicalPlan) -> (Plan, Vec<Row>) {
    let plan = PhysicalPlanner::new(st).plan(logical);
    let rows = execute(plan.clone(), st, NO_TRANSACTION).collect::<Result<Vec<Row>>>().unwrap();
    (plan, rows)
}

fn shown(row: &[Value]) -> String {
    row.iter().map(ToString::to_string).collect::<Vec<_>>().join("|")
}

/// Each row of `rows`, of `T`'s two columns then an id, is the row its
/// table keeps under that id.
fn ids_name_their_rows(st: &State, rows: &[Row]) {
    let storage = st.storage_of(&table()).unwrap();
    for row in rows {
        let (columns, id) = row.split_at(2);
        let id = row_id_of(id.first().unwrap()).unwrap().unwrap();
        assert_eq!(shown(&storage.get(NO_TRANSACTION, id).unwrap().unwrap()), shown(columns));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a]; [DESIGN] a located scan
/// of either engine follows each row with its id, by which its table
/// gives that row, each id once; one not located gives the row alone, so
/// `located` alone decides it.
#[test]
fn a_located_scan_follows_each_row_with_its_id() {
    for engine in [ROW_STORE, COLUMN_STORE] {
        let st = filled(engine);
        let (_, plain) = run(&st, &scan(false));
        assert!(plain.iter().all(|r| r.len() == 2), "{engine}");
        let (_, rows) = run(&st, &scan(true));
        assert_eq!(rows.len(), plain.len(), "{engine}");
        ids_name_their_rows(&st, &rows);
        let mut ids: Vec<String> = rows.iter().filter_map(|r| r.get(2)).map(ToString::to_string).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), rows.len(), "{engine}: each id once");
    }
}

/// Verifies: [DESIGN] RULES.md D31, D38: a located scan in an order
/// asked for, one told a filter's simple conditions, and a projection over
/// one, which would otherwise be read by batches, each keep the ids.
#[test]
fn ordered_filtered_and_projected_located_scans_keep_their_ids() {
    for engine in [ROW_STORE, COLUMN_STORE] {
        let st = filled(engine);
        let mut ordered = PhysicalPlanner::new(&st).plan(&scan(true));
        if let Plan::SeqScan { order, backward, .. } = &mut ordered {
            *order = vec![0];
            *backward = true;
        }
        let backward = execute(ordered, &st, NO_TRANSACTION).collect::<Result<Vec<Row>>>().unwrap();
        assert_eq!(backward.first().and_then(|r| r.first()).map(ToString::to_string), Some((ROWS - 1_i32).to_string()), "{engine}");
        ids_name_their_rows(&st, &backward);
        // V has no index, so the filter's condition is pushed into the scan.
        let under = Cond::Compare(Expr::Column(1), CompOp::Lt, int(100));
        let (plan, under) = run(&st, &LogicalPlan::Filter { cond: under, input: Box::new(scan(true)) });
        assert!(matches!(&plan, Plan::Filter { input, .. } if matches!(**input, Plan::SeqScan { located: true, .. })), "{plan:?}");
        assert_eq!(under.len(), 10, "{engine}");
        ids_name_their_rows(&st, &under);
        let swapped = LogicalPlan::Project { exprs: vec![Expr::Column(0), Expr::Column(1), Expr::Column(2)], input: Box::new(scan(true)) };
        let (_, projected) = run(&st, &swapped);
        assert_eq!(projected.len(), usize::try_from(ROWS).unwrap(), "{engine}");
        ids_name_their_rows(&st, &projected);
    }
}

/// Verifies: [DESIGN] RULES.md D35: a located read an index answers
/// follows the row it finds with its id.
#[test]
fn a_located_index_scan_follows_its_row_with_its_id() {
    for engine in [ROW_STORE, COLUMN_STORE] {
        let st = filled(engine);
        let equal = Cond::Compare(Expr::Column(0), CompOp::Eq, int(FOUND));
        let (plan, rows) = run(&st, &LogicalPlan::Filter { cond: equal, input: Box::new(scan(true)) });
        assert!(matches!(plan, Plan::IndexScan { located: true, .. }), "{plan:?}");
        assert_eq!(rows.len(), 1, "{engine}");
        ids_name_their_rows(&st, &rows);
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR5]; [DESIGN] RULES.md D35: a
/// LEFT index join of a located table follows each row it finds with its
/// id, and a left row it finds none for with nulls, the id's too.
#[test]
fn a_located_left_index_join_gives_ids_and_nulls() {
    for engine in [ROW_STORE, COLUMN_STORE] {
        let st = filled(engine);
        let left = LogicalPlan::Values {
            rows: vec![vec![int(FOUND)], vec![int(MISSING)]],
            types: vec![DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer }],
        };
        let join = LogicalPlan::OuterJoin {
            kind: OuterJoin::Left,
            cond: Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Column(1)),
            left: Box::new(left),
            right: Box::new(scan(true)),
        };
        let (plan, rows) = run(&st, &join);
        assert!(matches!(plan, Plan::IndexJoin { located: true, width: 3, .. }), "{plan:?}");
        let [found, missing] = rows.as_slice() else { panic!("{rows:?}") };
        let (_, table_part) = found.split_at(1);
        ids_name_their_rows(&st, &[table_part.to_vec()]);
        assert_eq!(shown(missing), format!("{MISSING}|NULL|NULL|NULL"), "{engine}");
    }
}
