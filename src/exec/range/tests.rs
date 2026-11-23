//! Unit tests of direct range reads. A plan of one range of an index,
//! under filters, projections, a sort, FETCH FIRST or set functions, is
//! one and gives the rows and errors its plan gives through the executor,
//! and any other plan is none (RULES.md R-TEST-1).

use std::sync::Arc;

use super::{At, RangeRead};
use crate::db::{Database, Session, State};
use crate::error::Result;
use crate::exec::{Runner, execute};
use crate::physical::{PhysicalPlanner, Plan};
use crate::storage::Row;
use crate::txn::NO_TRANSACTION;

/// A database of `A.T` (W, D, K, N, V, S), its primary key (W, D, K), of
/// 120 rows of each of 20 districts, N null of every seventh and S one of
/// thirteen names, with an index by name, and `A.U` (K, V) with no index.
fn filled() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (W INTEGER NOT NULL, D INTEGER NOT NULL, K INTEGER NOT NULL, N INTEGER, V DECIMAL(8,2), S VARCHAR(12),
             PRIMARY KEY (W, D, K))
           CREATE TABLE U (K INTEGER, V INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    for w in 1..=2_i64 {
        for d in 1..=10_i64 {
            let row = |k: i64| {
                let n = if k % 7 == 0 { "NULL".to_owned() } else { (k % 5).to_string() };
                format!("({w}, {d}, {k}, {n}, {}.50, 'S{}')", k.saturating_mul(d), k % 13)
            };
            let rows: Vec<String> = (1..=120_i64).map(row).collect();
            a.run(&format!("INSERT INTO T VALUES {}", rows.join(", "))).unwrap();
        }
    }
    a.run("INSERT INTO U VALUES (1, 1), (2, 2)").unwrap();
    a.commit().unwrap();
    a.run("CREATE INDEX T_S ON T (W, D, S, K)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The state `sql` is planned in, and its physical plan.
fn planned(s: &Session, sql: &str) -> (Arc<State>, Plan) {
    let (st, optimized) = s.optimized(sql).unwrap();
    let plan = PhysicalPlanner::new(&st).plan(&optimized);
    (st, plan)
}

/// Rows, or their error's SQLSTATE, as text.
fn shown(rows: Result<Vec<Row>>) -> Vec<String> {
    match rows {
        Ok(rows) => rows.iter().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>().join("|")).collect(),
        Err(e) => vec![format!("error {}", e.sqlstate().as_str())],
    }
}

/// What `sql` gives through the executor, and read direct, if its plan is
/// a direct read: all its rows, or the first `limit`.
fn both(s: &Session, sql: &str, limit: Option<usize>) -> (Vec<String>, Option<Vec<String>>) {
    let (st, plan) = planned(s, sql);
    let range = RangeRead::of(&plan, &st);
    let runner = Runner::new(&st, NO_TRANSACTION);
    let at = At { st: &st, tx: NO_TRANSACTION, values: &[], subqueries: &runner };
    let direct = range.map(|range| shown(range.rows(&at, limit).map(|rows| rows.expect("read direct"))));
    let rows = execute(plan, &st, NO_TRANSACTION);
    let through = match limit {
        Some(n) => rows.take(n).collect::<Result<Vec<Row>>>(),
        None => rows.collect::<Result<Vec<Row>>>(),
    };
    (shown(through), direct)
}

/// Verifies: [DESIGN] RULES.md D930: a plan of one range of an index is a
/// direct read, and gives the rows its plan gives through the executor,
/// in their order: a range alone and reversed, bounded on its next
/// column, filtered beside it, projected to columns, to one twice and to
/// values computed, its first rows after an offset, sorted by a column
/// the index does not order, and its set functions, DISTINCT and
/// filtered ones among them, of many rows, of one and of none.
#[test]
fn a_range_of_an_index_is_read_as_its_plan_reads() {
    let (_db, a) = filled();
    let queries = [
        "SELECT * FROM T WHERE W = 1 AND D = 3",
        "SELECT K, V FROM T WHERE W = 1 AND D = 3 ORDER BY K DESC",
        "SELECT K, S, K FROM T WHERE W = 2 AND D = 4 AND K >= 30 AND K < 41",
        "SELECT K, N FROM T WHERE W = 2 AND D = 4 AND K > 100 AND N = 2",
        "SELECT K + 1, V * 2, S || 'x' FROM T WHERE W = 1 AND D = 1 AND K <= 4",
        "SELECT K, V FROM T WHERE W = 1 AND D = 5 ORDER BY K OFFSET 3 ROWS FETCH FIRST 4 ROWS ONLY",
        "SELECT K FROM T WHERE W = 1 AND D = 5 AND N = 1 ORDER BY K DESC FETCH FIRST 1 ROW ONLY",
        "SELECT K, N, V FROM T WHERE W = 1 AND D = 2 AND K < 40 ORDER BY N DESC, V",
        "SELECT K, N FROM T WHERE W = 1 AND D = 2 AND K < 40 ORDER BY N, K DESC FETCH FIRST 5 ROWS ONLY",
        "SELECT K FROM T WHERE W = 1 AND D = 2 AND S = 'S4' ORDER BY K",
        "SELECT COUNT(*), COUNT(N), SUM(V), MIN(K), MAX(S), AVG(N) FROM T WHERE W = 1 AND D = 7",
        "SELECT COUNT(DISTINCT N) FROM T WHERE W = 2 AND D = 7 AND K > 60",
        "SELECT COUNT(*) FILTER (WHERE N > 2), SUM(V) FILTER (WHERE K < 10) FROM T WHERE W = 2 AND D = 8",
        "SELECT COUNT(S), MIN(K) FROM T WHERE W = 1 AND D = 9 AND S = 'S12'",
        "SELECT MIN(K) FROM T WHERE W = 1 AND D = 9 AND K > 119",
        "SELECT COUNT(*), SUM(V), MIN(K) FROM T WHERE W = 1 AND D = 9 AND K > 120",
        "SELECT COUNT(*) FROM T WHERE W = 3 AND D = 1",
        "SELECT K FROM T WHERE W = 3 AND D = 1",
        "SELECT N, COUNT(*), MAX(K) FROM T WHERE W = 1 AND D = 3 GROUP BY N",
        "SELECT S, SUM(V) FROM T WHERE W = 1 AND D = 3 AND S < 'S3' GROUP BY S",
    ];
    for sql in queries {
        let (through, direct) = both(&a, sql, None);
        assert_eq!(direct.unwrap_or_else(|| panic!("{sql} is read direct")), through, "{sql}");
    }
    assert_eq!(both(&a, queries[13], None).0, ["9|12"]);
    assert_eq!(both(&a, queries[15], None).0, ["0|NULL|NULL"]);
    assert_eq!(both(&a, queries[6], None).0, ["116"]);
}

/// Verifies: [DESIGN] RULES.md D930: a direct read asked for its first
/// rows reads no row past them, as the executor pulls no more, so an
/// exception a later row would raise is not raised; asked for all, it
/// raises that exception as the executor does (SQLSTATE 22012). FETCH
/// FIRST over a projection that can raise reads its input to its end, and
/// raises too [ISO/IEC 9075:1992 §3.3.4.4].
#[test]
fn a_direct_read_raises_what_its_plan_raises_and_reads_no_further() {
    let (_db, a) = filled();
    let divided = "SELECT 10 / (K - 3) FROM T WHERE W = 1 AND D = 1";
    for limit in [Some(0), Some(1), Some(2), Some(3), None] {
        let (through, direct) = both(&a, divided, limit);
        assert_eq!(direct.expect("read direct"), through, "{limit:?}");
    }
    assert_eq!(both(&a, divided, Some(2)).0, ["-5", "-10"]);
    assert_eq!(both(&a, divided, None).0, ["error 22012"]);
    let first = "SELECT 10 / (K - 3) FROM T WHERE W = 1 AND D = 1 FETCH FIRST 2 ROWS ONLY";
    let (through, direct) = both(&a, first, None);
    assert_eq!((direct.expect("read direct"), through.as_slice()), (through.clone(), ["error 22012".to_owned()].as_slice()));
}

/// Verifies: [DESIGN] RULES.md D930: a plan that reads no index, groups
/// by a column, removes duplicates, joins, keeps a percentage or the
/// peers of its last row, or whose condition or values read a subquery,
/// is no direct read.
#[test]
fn a_plan_of_another_shape_is_no_direct_read() {
    let (_db, a) = filled();
    let others = [
        "SELECT K FROM U WHERE K = 1",
        "SELECT DISTINCT N FROM T WHERE W = 1 AND D = 3",
        "SELECT T.K, U.V FROM T, U WHERE W = 1 AND D = 3 AND T.K = U.K",
        "SELECT K FROM T WHERE W = 1 AND D = 3 ORDER BY K FETCH FIRST 10 PERCENT ROWS ONLY",
        "SELECT K, N FROM T WHERE W = 1 AND D = 3 ORDER BY N FETCH FIRST 2 ROWS WITH TIES",
        "SELECT K FROM T WHERE W = 1 AND D = 3 AND K = (SELECT MAX(K) FROM U)",
        "SELECT K, (SELECT MAX(V) FROM U) FROM T WHERE W = 1 AND D = 3",
    ];
    for sql in others {
        let (st, plan) = planned(&a, sql);
        assert!(RangeRead::of(&plan, &st).is_none(), "{sql}: {plan:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D930, D931: a direct read says whether its
/// plan can raise an exception, and whether it holds every row before it
/// gives one, as a cursor of it is read at OPEN by; and it counts its
/// search and entries for its index as the scan does (D522).
#[test]
fn a_direct_read_says_how_a_cursor_reads_it_and_counts_its_entries() {
    let (_db, a) = filled();
    let said = |sql: &str| {
        let (st, plan) = planned(&a, sql);
        let range = RangeRead::of(&plan, &st).unwrap_or_else(|| panic!("{sql} is read direct"));
        (range.raises(), range.holds())
    };
    assert_eq!(said("SELECT K, V FROM T WHERE W = 1 AND D = 3"), (false, false));
    assert_eq!(said("SELECT K / N FROM T WHERE W = 1 AND D = 3"), (true, false));
    assert_eq!(said("SELECT K, N FROM T WHERE W = 1 AND D = 3 ORDER BY N"), (false, true));
    assert!(said("SELECT COUNT(*) FROM T WHERE W = 1 AND D = 3").1);
    let (st, plan) = planned(&a, "SELECT K, N FROM T WHERE W = 1 AND D = 3 AND K <= 50");
    let range = RangeRead::of(&plan, &st).unwrap();
    let mut scan = &plan;
    while let Plan::Project { input, .. } | Plan::Filter { input, .. } = scan {
        scan = input;
    }
    let Plan::IndexScan { index, .. } = scan else { panic!("an index scan: {plan:?}") };
    let before = st.index_uses[index].totals();
    let runner = Runner::new(&st, NO_TRANSACTION);
    let at = At { st: &st, tx: NO_TRANSACTION, values: &[], subqueries: &runner };
    assert_eq!(range.rows(&at, None).unwrap().unwrap().len(), 50);
    let after = st.index_uses[index].totals();
    assert_eq!((after.0 - before.0, after.1 - before.1), (1, 50), "one search of fifty entries");
}

/// Verifies: [DESIGN] RULES.md D930: a plan kept is run as its direct
/// range read while EXECUTOR.POINT_OPERATIONS is on, and through the
/// executor once it is off.
#[test]
fn point_operations_off_run_a_kept_range_through_the_executor() {
    let (db, a) = filled();
    let (st, plan) = planned(&a, "SELECT K, V FROM T WHERE W = 1 AND D = 3");
    let kept = crate::physical::prepared_plan(plan.clone(), None).unwrap();
    assert!(kept.range(&st).is_some());
    db.set_setting("EXECUTOR.POINT_OPERATIONS", &crate::value::Value::Char("OFF".into())).unwrap();
    let off = db.snapshot().unwrap();
    let off_kept = crate::physical::prepared_plan(plan, None).unwrap();
    assert!(off_kept.range(&off).is_none());
}
