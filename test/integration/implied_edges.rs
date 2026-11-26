//! \[DESIGN\] The edges a class of equalities implies (RULES.md D1204): a
//! relation whose column of a class is a key of it is joined to the
//! class's other relations by the equality the class implies, where that
//! is cheaper, and the rows are those of the query as written
//! [X3.135-1986 §5.20 GR2b]. These are requirements on the plan; the
//! standard governs only the rows.

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::{exec, sorted};

/// The parts P0 has, the suppliers of each in PS, and the lines of each
/// of those in L.
const PARTS: i64 = 400;
/// The parts of F = 0, one in 20 of them.
const KEPT: i64 = 20;
const SUPPLIERS: i64 = 4;
const LINES: i64 = 5;

/// Schema E: P0 of the parts, its column K declared as `key` says, a
/// twentieth of them of F = 0 and a few of a null K if it may be null; PS
/// of each part's suppliers, its key K and S; L of the lines of each, with
/// lines of a null K too.
fn parts(key: &str) -> Database {
    let db = Database::new();
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION E
           CREATE TABLE P0 ({key}, F INTEGER)
           CREATE TABLE PS (K INTEGER NOT NULL, S INTEGER NOT NULL, PRIMARY KEY (K, S))
           CREATE TABLE L (K INTEGER, S INTEGER, Q INTEGER)"
    ))
    .unwrap();
    let s = db.session("E");
    let list = |rows: Vec<String>| rows.join(", ");
    let nulls = if key.contains("NOT NULL") { String::new() } else { ", (NULL, 0), (NULL, 0)".to_owned() };
    s.run(&format!("INSERT INTO P0 VALUES {}{nulls}", list((0..PARTS).map(|k| format!("({k}, {})", k % 20)).collect()))).unwrap();
    let supplied = (0..PARTS).flat_map(|k| (0..SUPPLIERS).map(move |x| format!("({k}, {x})")));
    s.run(&format!("INSERT INTO PS VALUES {}", list(supplied.collect()))).unwrap();
    for k in 0..PARTS {
        let lines = (0..SUPPLIERS).flat_map(|x| (0..LINES).map(move |q| format!("({k}, {x}, {q})")));
        s.run(&format!("INSERT INTO L VALUES {}, (NULL, 0, 1)", list(lines.collect()))).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The lines of the parts of F = 0, P read as `p` says, the FROM clause
/// joining L to PS first.
fn lines_of(p: &str) -> String {
    format!("SELECT COUNT(*), SUM(L.Q) FROM L, PS, {p} WHERE P.K = L.K AND PS.K = L.K AND PS.S = L.S AND P.F = 0")
}

/// P0 as P.
const TABLE: &str = "P0 AS P";

/// Whether the plan of `sql` joins P and PS to each other before L, and
/// the plan.
fn joins_p_to_ps(s: &Session, sql: &str) -> (bool, String) {
    let plan = s.explain(sql, &Params::new()).unwrap();
    assert!(plan.contains("(join order by DPccp: "), "{plan}");
    // A derived table is shown by the table its first column is of.
    let joined = ["(P PS)", "(PS P)", "(P0 PS)", "(PS P0)"].iter().any(|pair| plan.contains(pair));
    (joined, plan)
}

/// The rows of `sql` planned adaptively, which the greedy order's must be.
fn agreed(s: &Session, sql: &str) -> Vec<String> {
    let rows = sorted(s, sql);
    exec(s, "SET PLANNER.JOIN_ENUMERATION = 'GREEDY'").unwrap();
    assert_eq!(sorted(s, sql), rows, "{sql}: greedy and adaptive");
    exec(s, "SET PLANNER.JOIN_ENUMERATION = 'ADAPTIVE'").unwrap();
    rows
}

/// The count and the sum of Q of the lines of the parts of F = 0.
fn expected() -> Vec<String> {
    let lines = KEPT * SUPPLIERS * LINES;
    vec![format!("{lines},{}", KEPT * SUPPLIERS * (0..LINES).sum::<i64>())]
}

/// Verifies: [DESIGN] RULES.md D1204: P's column a PRIMARY KEY, the
/// filtered P is joined to PS by the equality `P.K = L.K AND PS.K = L.K`
/// imply, before L, and the rows are the greedy order's; a UNIQUE column
/// that may be null, with rows of a null in it, likewise, as a null
/// equals nothing [X3.135-1986 §5.11 GR2].
#[test]
fn a_declared_key_of_a_class_joins_its_other_relations() {
    for key in ["K INTEGER NOT NULL PRIMARY KEY", "K INTEGER UNIQUE"] {
        let db = parts(key);
        let s = db.session("E");
        let (joined, plan) = joins_p_to_ps(&s, &lines_of(TABLE));
        assert!(joined, "{key}\n{plan}");
        assert_eq!(agreed(&s, &lines_of(TABLE)), expected(), "{key}");
    }
}

/// Verifies: [DESIGN] RULES.md D1204, D384: a relation that groups by the
/// class's column alone, or is DISTINCT of that column alone, holds a key
/// of it too, and is joined to PS first, the rows the greedy order's.
#[test]
fn a_grouping_or_distinct_column_of_a_class_is_a_key() {
    let db = parts("K INTEGER");
    let s = db.session("E");
    let grouped = lines_of("(SELECT K, MIN(F) FROM P0 GROUP BY K) AS P (K, F)");
    let distinct = lines_of("(SELECT DISTINCT K FROM P0 WHERE F = 0) AS P").replace(" AND P.F = 0", "");
    for sql in [grouped, distinct] {
        let (joined, plan) = joins_p_to_ps(&s, &sql);
        assert!(joined, "{sql}\n{plan}");
        assert_eq!(agreed(&s, &sql), expected(), "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D1204: no edge is implied from a column
/// that is no key of its relation, nor from a key whose constraint is
/// deferrable, which may be broken until it is checked [ISO/IEC
/// 9075-2:2023 §4.25.2]; P and PS are then joined only through L, and the
/// rows are the same. With the deferred key broken in an open
/// transaction, two parts of one K, the lines of that K are counted for
/// each of the two.
#[test]
fn no_edge_is_implied_without_an_immediate_key() {
    for key in ["K INTEGER", "K INTEGER CONSTRAINT LATE UNIQUE DEFERRABLE INITIALLY DEFERRED"] {
        let db = parts(key);
        let s = db.session("E");
        let (joined, plan) = joins_p_to_ps(&s, &lines_of(TABLE));
        assert!(!joined, "{key}\n{plan}");
        assert_eq!(agreed(&s, &lines_of(TABLE)), expected(), "{key}");
        s.run("INSERT INTO P0 VALUES (0, 0)").unwrap();
        let lines = (KEPT + 1) * SUPPLIERS * LINES;
        let twice = vec![format!("{lines},{}", (KEPT + 1) * SUPPLIERS * (0..LINES).sum::<i64>())];
        assert_eq!(agreed(&s, &lines_of(TABLE)), twice, "{key}");
        s.rollback().unwrap();
    }
}

/// Verifies: [X3.135-1986 §5.11 GR4]; [DESIGN] RULES.md D1204, D387: an
/// equality a class implies is true of every row the stated ones keep,
/// across exact and approximate numbers too, as numbers compare by
/// algebraic value. A.X = 2^53 + 1 equals no DOUBLE PRECISION value, so
/// `A.X = B.D AND B.D = C.Y` keeps no row of it though C.Y = 2^53 = B.D,
/// A joined to C by the implied key or not; with A.X = 2^53 it keeps one.
#[test]
fn an_implied_equality_of_exact_and_approximate_numbers_keeps_the_rows() {
    for (x, more) in [("9007199254740993", 0_i64), ("9007199254740992", 1)] {
        let db = Database::new();
        db.schema(
            "CREATE SCHEMA AUTHORIZATION N
               CREATE TABLE A (X DECIMAL(20, 0) NOT NULL PRIMARY KEY)
               CREATE TABLE B (D DOUBLE PRECISION, W INTEGER)
               CREATE TABLE C (Y DECIMAL(20, 0))",
        )
        .unwrap();
        let s = db.session("N");
        let small: Vec<String> = (0..40_i64).map(|k| format!("({k})")).collect();
        s.run(&format!("INSERT INTO A VALUES ({x}), {}", small.join(", "))).unwrap();
        s.run(&format!("INSERT INTO C VALUES (9007199254740992), {}", small.join(", "))).unwrap();
        let wide: Vec<String> = (0..2000_i64).map(|k| format!("({}E0, {k})", k % 40)).collect();
        s.run(&format!("INSERT INTO B VALUES (9007199254740992E0, 0), {}", wide.join(", "))).unwrap();
        s.commit().unwrap();
        db.maintain().unwrap();
        let sql = "SELECT COUNT(*) FROM B, A, C WHERE A.X = B.D AND B.D = C.Y";
        let plan = s.explain(sql, &Params::new()).unwrap();
        assert!(plan.contains("(A C)") || plan.contains("(C A)"), "{plan}");
        assert_eq!(agreed(&s, sql), vec![(2000 + more).to_string()], "{x}");
    }
}
