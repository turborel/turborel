//! Subqueries of existence and quantified comparisons made mark joins
//! (RULES.md D640 to D643): each query's rows are those of the subquery
//! run for each row, and of the subquery unnested as an outer join of
//! counted bindings, by batches and row by row, held in memory and
//! spilled.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::optimizer::MARK_JOINS;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

use super::{both, outcome, random_tables, unnested};
use crate::common::set_rule;

/// The forms made mark joins, over R and S of `random_tables`, each with
/// the operator EXPLAIN shows. Those of a select list name the mark's
/// place M.
const MARKED: &[(&str, &str)] = &[
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K) OR V > 2", "HashMarkJoin"),
    ("SELECT K, V FROM R WHERE NOT EXISTS (SELECT * FROM S WHERE S.K = R.K) OR V IS NULL", "HashMarkJoin"),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W > R.V) OR K < 0", "HashMarkJoin"),
    ("SELECT K, V FROM R WHERE V IN (SELECT W FROM S WHERE S.K = R.K) OR K IS NULL", "HashMarkJoin"),
    ("SELECT K, V FROM R WHERE V NOT IN (SELECT W FROM S WHERE S.K = R.K) OR K = 1", "HashMarkJoin"),
    ("SELECT K, V FROM R WHERE NOT (V = ANY (SELECT W FROM S WHERE S.K = R.K) OR V > 3)", "HashMarkJoin"),
    ("SELECT K, (EXISTS (SELECT * FROM S WHERE S.K = R.K)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.C = R.C)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V IN (SELECT W FROM S WHERE S.K = R.K)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V NOT IN (SELECT W FROM S WHERE S.K = R.K)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V <> ALL (SELECT W FROM S WHERE S.K = R.K AND S.W > 0)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, ((K, V) IN (SELECT K, W FROM S WHERE S.C = R.C)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, ((K, V) NOT IN (SELECT K, W FROM S WHERE S.C = R.C)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (C IN (SELECT C FROM S WHERE S.K = R.K)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V + 1 IN (SELECT W FROM S WHERE S.K = R.K)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V > ALL (SELECT W FROM S WHERE S.K = R.K)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V = ALL (SELECT W FROM S WHERE S.K = R.K)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V <> SOME (SELECT W FROM S WHERE S.K = R.K)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V <= SOME (SELECT W FROM S WHERE S.K = R.K)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V >= ALL (SELECT W FROM S WHERE S.C = R.C)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V < SOME (SELECT W FROM S WHERE S.W > R.V)) AS M FROM R", "NestedLoopMarkJoin"),
    ("SELECT K, (EXISTS (SELECT * FROM S WHERE S.W > R.V)) AS M FROM R", "NestedLoopMarkJoin"),
    ("SELECT K, (V IN (SELECT W FROM S WHERE S.K = R.K)) IS UNKNOWN AS M FROM R", "HashMarkJoin"),
    ("SELECT K, CASE WHEN EXISTS (SELECT * FROM S WHERE S.K = R.K) THEN 'y' ELSE 'n' END AS M FROM R", "HashMarkJoin"),
    ("SELECT K, CASE WHEN K > 1 THEN (V IN (SELECT W FROM S WHERE S.K = R.K)) ELSE FALSE END AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V IN (SELECT MAX(W) FROM S WHERE S.K = R.K GROUP BY S.C)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (EXISTS (SELECT S.C FROM S WHERE S.K = R.K GROUP BY S.C HAVING COUNT(*) > 1)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K, (V IN (SELECT W FROM S WHERE S.K = R.K)) AND (EXISTS (SELECT * FROM S WHERE S.W = R.V)) AS M FROM R", "HashMarkJoin"),
    ("SELECT K FROM R GROUP BY K HAVING EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W > 3) OR K > 2", "HashMarkJoin"),
];

/// The physical plan of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    let plan = s.explain(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    plan.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// The rows of `sql` with OPTIMIZER.MARK_JOINS OFF, unnested otherwise.
fn counted_otherwise(db: &Database, s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    set_rule(db, MARK_JOINS, false);
    let rows = outcome(s, sql);
    set_rule(db, MARK_JOINS, true);
    rows
}

/// The rows of `sql` read by batches, grouped by a set function as it
/// reads them, and read row by row (EXECUTOR.BATCHES OFF).
fn both_paths(s: &Session, sql: &str) -> (Result<Vec<String>, Code>, Result<Vec<String>, Code>) {
    let grouped = if sql.contains(" AS M ") {
        format!("SELECT M, COUNT(*), MIN(K), MAX(K) FROM ({sql}) AS Q GROUP BY M")
    } else {
        format!("SELECT COUNT(*), MIN(K), MAX(K) FROM ({sql}) AS Q")
    };
    let batched = (outcome(s, &grouped), outcome(s, sql));
    s.run("SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let rows = (outcome(s, &grouped), outcome(s, sql));
    s.run("SET EXECUTOR.BATCHES = 'ON'").unwrap();
    assert_eq!(batched.1, rows.1, "by batches: {sql}");
    (batched.0, rows.0)
}

/// Checks each form of `MARKED` over `db`: its plan, and its rows as the
/// subquery run for each row gives them, as counted bindings do, and by
/// batches as row by row.
fn checked(db: &Database, s: &Session, spilled: bool) {
    for (q, op) in MARKED {
        if !spilled {
            let shown = physical(s, q);
            assert!(shown.contains(op) && !shown.contains("Subquery"), "{q}\n{shown}");
            assert!(unnested(s, q), "{q}");
        }
        let (on, off) = both(db, s, q);
        assert_eq!(on, off, "{q}");
        assert_eq!(on, counted_otherwise(db, s, q), "{q}");
        let (batched, rows) = both_paths(s, q);
        assert_eq!(batched, rows, "by batches: {q}");
    }
}

/// Verifies: [X3.135-1986 §5.16 GR2, §5.17 GR2; ISO/IEC 9075:1992 §8.7
/// GR2, §8.12 GR2] (RULES.md D640, D641, D642, D643) every form made a
/// mark join, EXISTS and quantified comparisons under OR and NOT, in a
/// select list, in CASE and in HAVING, shows it in EXPLAIN and has the
/// rows the subquery has run for each row, and unnested as an outer join
/// of counted bindings: over random tables of nulls, duplicates and
/// trailing spaces, with indexes and without, by batches and row by row,
/// and with a work area at its least.
#[test]
fn mark_joins_match_the_subqueries() {
    for seed in [0x3A2C_0000_0000_0001, 0x3A2C_0000_0000_0002, 0x3A2C_0000_0000_0003] {
        let db = random_tables(seed);
        let s = db.session("A");
        checked(&db, &s, false);
        s.commit().unwrap();
        s.run("CREATE INDEX SK ON S (K)").unwrap();
        s.commit().unwrap();
        checked(&db, &s, false);
        db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(65_536, 0))).unwrap();
        checked(&db, &s, true);
    }
}

/// Verifies: [X3.135-1986 §5.16 GR2, §5.17 GR2; ISO/IEC 9075:1992 §8.2
/// GR7a, b, g, m, §8.7 GR2] (RULES.md D642) a mark's three values with nulls on
/// each side and right sides empty: IN is true of an equal value, unknown
/// of a null among the values or of a null value of a set not empty,
/// false of an empty set even of a null value; NOT IN and <> ALL their
/// negations; a row's IN false where one value differs, though another is
/// null, and unknown where none differs but one is null; EXISTS true or
/// false alone. Each as the subquery run for each
/// row gives it, the rule on and off, held in memory and spilled.
#[test]
fn marks_of_nulls_and_empty_sets() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (K INTEGER, V INTEGER) CREATE TABLE S (K INTEGER, W INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO R VALUES (1, 1), (1, 2), (1, NULL), (2, 1), (2, NULL), (3, 1), (3, NULL), (NULL, 1)").unwrap();
    s.run("INSERT INTO S VALUES (1, 1), (1, NULL), (2, 1), (2, 3), (4, 5), (NULL, 1)").unwrap();
    let all: [&str; 8] = ["-,[1]", "[1],-", "[1],[1]", "[1],[2]", "[2],-", "[2],[1]", "[3],-", "[3],[1]"];
    let cases: &[(&str, [&str; 8])] = &[
        (
            "SELECT K, V, (V IN (SELECT W FROM S WHERE S.K = R.K)) FROM R",
            ["[FALSE]", "-", "[TRUE]", "-", "-", "[TRUE]", "[FALSE]", "[FALSE]"],
        ),
        (
            "SELECT K, V, (V NOT IN (SELECT W FROM S WHERE S.K = R.K)) FROM R",
            ["[TRUE]", "-", "[FALSE]", "-", "-", "[FALSE]", "[TRUE]", "[TRUE]"],
        ),
        (
            "SELECT K, V, (V <> ALL (SELECT W FROM S WHERE S.K = R.K)) FROM R",
            ["[TRUE]", "-", "[FALSE]", "-", "-", "[FALSE]", "[TRUE]", "[TRUE]"],
        ),
        (
            "SELECT K, V, (V < ALL (SELECT W FROM S WHERE S.K = R.K)) FROM R",
            ["[TRUE]", "-", "[FALSE]", "[FALSE]", "-", "[FALSE]", "[TRUE]", "[TRUE]"],
        ),
        (
            "SELECT K, V, (V < SOME (SELECT W FROM S WHERE S.K = R.K)) FROM R",
            ["[FALSE]", "-", "-", "-", "-", "[TRUE]", "[FALSE]", "[FALSE]"],
        ),
        (
            "SELECT K, V, ((K, V) IN (SELECT K, W FROM S WHERE S.W = R.V OR S.W IS NULL)) FROM R",
            ["-", "-", "[TRUE]", "-", "[FALSE]", "[TRUE]", "[FALSE]", "-"],
        ),
        (
            "SELECT K, V, (EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W = R.V)) FROM R",
            ["[FALSE]", "[FALSE]", "[TRUE]", "[FALSE]", "[FALSE]", "[TRUE]", "[FALSE]", "[FALSE]"],
        ),
    ];
    marked_as(&db, &s, &all, cases);
    // Rows of keys and values no row of R has: none changes a mark, and
    // past a work area at its least the held rows spill.
    let filler: Vec<String> = (0..2_000).map(|n| format!("({}, {})", 100 + n, 1_000 + n)).collect();
    s.run(&format!("INSERT INTO S VALUES {}", filler.join(", "))).unwrap();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(65_536, 0))).unwrap();
    marked_as(&db, &s, &all, cases);
}

/// Checks that each query of `cases` gives the rows `all` their marks, the
/// rule on and off and unnested otherwise.
fn marked_as(db: &Database, s: &Session, all: &[&str; 8], cases: &[(&str, [&str; 8])]) {
    for (q, marks) in cases {
        let mut want: Vec<String> = all.iter().zip(marks).map(|(row, m)| format!("{row},{m}")).collect();
        want.sort();
        assert!(physical(s, q).contains("MarkJoin"), "{q}");
        let (on, off) = both(db, s, q);
        assert_eq!(on, Ok(want.clone()), "{q}");
        assert_eq!(off, Ok(want.clone()), "{q}");
        assert_eq!(counted_otherwise(db, s, q), Ok(want), "{q}");
    }
}

/// Verifies: [DESIGN] (RULES.md D641) with OPTIMIZER.MARK_JOINS OFF no
/// mark join is made and the subquery is unnested as before, an outer
/// join of each binding's counted rows; a subquery that can raise, which a
/// mark join may leave unread or read for bindings no row has, a value
/// compared that can raise where a CASE may leave it unevaluated, and a
/// subquery uncorrelated, which the executor reads once, are made none.
#[test]
fn mark_joins_switched_off_and_refused() {
    let db = random_tables(4);
    let s = db.session("A");
    let q = "SELECT K, (V IN (SELECT W FROM S WHERE S.K = R.K)) FROM R";
    set_rule(&db, MARK_JOINS, false);
    let shown = physical(&s, q);
    assert!(!shown.contains("MarkJoin") && shown.contains("LeftOuterJoin"), "{shown}");
    set_rule(&db, MARK_JOINS, true);
    for q in [
        "SELECT K, (EXISTS (SELECT * FROM S WHERE S.K = R.K AND 10 / S.W > 1)) FROM R",
        "SELECT K, (V IN (SELECT W FROM S)) FROM R",
        "SELECT K, (V IN (SELECT W - 1 FROM S WHERE S.K = R.K)) FROM R",
        "SELECT K, CASE WHEN K > 1 THEN (V + 1 IN (SELECT W FROM S WHERE S.K = R.K)) ELSE FALSE END FROM R",
        "SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K)",
    ] {
        assert!(!physical(&s, q).contains("MarkJoin"), "{q}");
        let (on, off) = both(&db, &s, q);
        assert_eq!(on, off, "{q}");
    }
}
