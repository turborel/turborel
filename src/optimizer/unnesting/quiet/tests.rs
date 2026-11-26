//! Unit tests of the judgment that a scalar subquery raises no error and
//! no warning: each refusal beside the subquery that passes it
//! (RULES.md R-TEST-1).

use super::quiet;
use crate::db::{Database, Session};
use crate::exec::execute;
use crate::logical::{LogicalPlan, Subquery};
use crate::physical::PhysicalPlanner;
use crate::txn::NO_TRANSACTION;
use crate::value::{Decimal, Value};

/// A session of `A.T` (K, B, N, S), K and B declared to hold no null.
fn session() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL, B DECIMAL(10,2) NOT NULL, N DECIMAL(10,2), S VARCHAR(8))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 1.50, NULL, 'AB'), (2, 2.50, 3.00, 'CD')").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The first subquery a node of `plan` holds, its inputs read after it.
fn first(plan: &LogicalPlan) -> Option<Subquery> {
    plan.subqueries().first().map(|s| (*s).clone()).or_else(|| plan.inputs().find_map(first))
}

/// Whether the subquery `subquery` raises nothing, as the filter of
/// `SELECT K FROM T WHERE B > (subquery) OR K < 0` holds it: an operand
/// of OR, so no semi join is made of it (RULES.md D773).
fn judged(a: &Session, subquery: &str) -> bool {
    let (st, optimized) = a.optimized(&format!("SELECT K FROM T WHERE B > ({subquery}) OR K < 0")).unwrap();
    let s = first(&optimized).expect("the filter keeps its subquery");
    quiet(&s, &st)
}

/// Verifies: [DESIGN] RULES.md D1132, D1060: the average of a column
/// declared to hold no null, or of one a filter below compares, over a
/// filter that raises nothing, raises nothing and warns of nothing; one
/// of a column that may hold a null warns that a null was eliminated, so
/// is not quiet.
#[test]
fn a_set_function_of_a_filled_column_is_quiet_and_one_of_nulls_is_not() {
    let (_db, a) = session();
    assert!(judged(&a, "SELECT AVG(B) FROM T WHERE B > 0.00 AND SUBSTRING(S FROM 1 FOR 1) IN ('A', 'C')"));
    assert!(judged(&a, "SELECT COUNT(*) FROM T"), "no argument, no null to eliminate");
    assert!(!judged(&a, "SELECT AVG(N) FROM T"), "a null of N is eliminated, with a warning");
    assert!(judged(&a, "SELECT AVG(N) FROM T WHERE N > 0.00"), "a filter comparing N keeps no null of it");
    assert!(judged(&a, "SELECT AVG(N) FROM T WHERE N IS NOT NULL"));
    assert!(!judged(&a, "SELECT AVG(N) FROM T WHERE N IS NULL OR K > 0"), "a disjunction may keep a null");
    assert!(!judged(&a, "SELECT AVG(B + N) FROM T"), "an argument not a column is not judged");
}

/// Verifies: [DESIGN] RULES.md D1132, D1060: a subquery that may raise
/// an error is not quiet: one dividing by a column, which may be zero,
/// and one that may give two rows.
#[test]
fn a_subquery_that_may_raise_is_not_quiet() {
    let (_db, a) = session();
    assert!(judged(&a, "SELECT MAX(B) FROM T"));
    assert!(!judged(&a, "SELECT MAX(B) FROM T WHERE 1 / K > 0"), "a division by K may raise");
    assert!(!judged(&a, "SELECT B FROM T"), "two rows raise a cardinality violation");
    assert!(judged(&a, "SELECT B FROM T FETCH FIRST 1 ROW ONLY"), "one row at most");
}

/// A session of `A.T` of 400 rows, its parts run on four threads in
/// morsels of 16 rows whatever their size.
fn parallel_session() -> (Database, Session) {
    let db = Database::new();
    let count = |n: i128| Value::Exact(Decimal::new(n, 0));
    for (name, n) in [("EXECUTOR.WORKERS", 4), ("EXECUTOR.DEGREE", 4), ("EXECUTOR.MORSEL_ROWS", 16), ("EXECUTOR.PARALLEL_MIN_ROWS", 0)] {
        db.set_setting(name, &count(n)).unwrap();
    }
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL, B DECIMAL(10,2) NOT NULL, N DECIMAL(10,2))").unwrap();
    let a = db.session("A");
    let rows: Vec<String> =
        (0..400_i64).map(|k| format!("({k}, {}.25, {})", k % 37, if k % 5 == 0 { "NULL".to_owned() } else { k.to_string() })).collect();
    a.run(&format!("INSERT INTO T VALUES {}", rows.join(", "))).unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The plan of `sql` as `a` runs it, its parts marked, and what it gives:
/// its rows' values sorted, or its error's SQLSTATE.
fn run(a: &Session, sql: &str) -> (String, Vec<String>) {
    let (st, optimized) = a.optimized(sql).unwrap();
    let plan = crate::physical::parallel(&st, &crate::physical::Seen::default(), PhysicalPlanner::new(&st).plan(&optimized));
    let shown = plan.explain();
    let gave = match execute(plan, &st, NO_TRANSACTION).collect::<crate::error::Result<Vec<_>>>() {
        Ok(rows) => {
            let mut out: Vec<String> = rows.iter().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>().join("|")).collect();
            out.sort();
            out
        }
        Err(e) => vec![format!("error {}", e.sqlstate().as_str())],
    };
    (shown, gave)
}

/// Whether the filter of the plan `shown` is within a part run in
/// parallel, below its mark.
fn in_part(shown: &str) -> bool {
    shown.find("Parallel").zip(shown.find("Filter")).is_some_and(|(part, filter)| part < filter)
}

/// Verifies: [DESIGN] RULES.md D1132, D15: a filter holding a quiet
/// scalar subquery is a step of a part run in parallel, its subquery
/// bound once before the morsels start, and keeps the rows the filter
/// judged a row at a time keeps; one whose subquery warns, or raises, is
/// kept off the part, and the raising one raises what it raised.
#[test]
fn a_filter_of_a_quiet_subquery_runs_in_parallel_and_others_do_not() {
    let (_db, a) = parallel_session();
    let (shown, gave) = run(&a, "SELECT K FROM T WHERE B > (SELECT AVG(B) FROM T) OR K < 0");
    assert!(in_part(&shown), "a quiet subquery's filter is in the part: {shown}");
    let mut kept: Vec<String> = (0..400_i64).filter(|k| k % 37 > 17).map(|k| k.to_string()).collect();
    kept.sort();
    assert_eq!(gave, kept, "the rows above the average, 17.98");
    let (warned, _) = run(&a, "SELECT K FROM T WHERE B > (SELECT AVG(N) FROM T) OR K < 0");
    assert!(!in_part(&warned), "a subquery that warns keeps its filter serial: {warned}");
    let (raised, error) = run(&a, "SELECT K FROM T WHERE B > (SELECT MAX(B) FROM T WHERE 1 / K > 0) OR K < 0");
    assert!(!in_part(&raised), "a subquery that may raise keeps its filter serial: {raised}");
    assert_eq!(error, vec!["error 22012".to_owned()], "K of 0 divides by zero");
}

/// Whether the optimized plan of `sql` joins a subquery (RULES.md D773).
fn joins(a: &Session, sql: &str) -> bool {
    let (_, optimized) = a.optimized(sql).unwrap();
    format!("{optimized:?}").contains("SemiJoin")
}

/// Verifies: [DESIGN] RULES.md D1132, D773: a comparison with a quiet
/// subquery is no semi join but a filter's, unless the column compared is
/// one an index holds, which the join may probe; a subquery that warns is
/// joined as before.
#[test]
fn a_quiet_subquery_is_joined_only_to_probe_an_index() {
    let (_db, a) = session();
    assert!(!joins(&a, "SELECT K FROM T WHERE B > (SELECT AVG(B) FROM T)"), "compared as a constant");
    assert!(joins(&a, "SELECT K FROM T WHERE B > (SELECT AVG(N) FROM T)"), "a subquery that warns is joined");
    a.run("CREATE INDEX T_B ON T (B)").unwrap();
    a.commit().unwrap();
    assert!(joins(&a, "SELECT K FROM T WHERE B > (SELECT AVG(B) FROM T)"), "B is indexed, so probed");
}
