//! \[EXTENSION\] The planner's plug-ins of RULES.md D275 to D279, each
//! served by a real process of the example plug-in
//! (`plugin/examples/plugin_example`): optimizer rules over the plan
//! form, a rule skipped when it fails, statistics a storage engine and an
//! index method give the planner, and statistics a procedure asks of the
//! database.

use turborel::db::{Completion, Database, Session};
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::{Scratch, rows};
use crate::plugins::fixture::{allow, install, manifest};

/// A scratch directory whose plug-ins hold PLANNER at 1.0.0 of `objects`,
/// and a database in memory allowed it, PLANNER created by U, who owns
/// T (A, B) of 20 rows, A from 1 to 20 and B its remainder by 4.
fn planner(test: &str, objects: &str) -> (Scratch, Database) {
    let scratch = Scratch::new(test);
    let root = scratch.path("plugins");
    install(&root, ("PLANNER", "1.0.0"), &manifest("PLANNER", "1.0.0", "", objects));
    let db = Database::new();
    allow(&db, &root);
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (A INTEGER, B INTEGER)").unwrap();
    let s = db.session("U");
    for a in 1..=20 {
        s.run(&format!("INSERT INTO T VALUES ({a}, {})", a % 4)).unwrap();
    }
    s.run("CREATE EXTENSION planner").unwrap();
    s.commit().unwrap();
    (scratch, db)
}

/// The calls made of extension PLANNER so far.
fn calls(s: &Session) -> u64 {
    rows(s, "SELECT CALLS FROM INFORMATION_SCHEMA.EXTENSIONS WHERE EXTENSION_NAME = 'PLANNER'").unwrap()[0].parse().unwrap()
}

/// EXPLAIN of `query`.
fn explained(s: &Session, query: &str) -> String {
    s.explain(query, &Params::new()).unwrap()
}

/// The completion of `query`, a single row's, read into N.
fn completed(s: &Session, query: &str) -> Completion {
    let exact = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let mut p = Params::new().declare("N", exact, Value::Null);
    s.execute(query, &mut p).unwrap()
}

/// The queries the rules see, each with its rows.
const QUERIES: [&str; 3] = [
    "SELECT A FROM T WHERE A > 5 AND A < 3 ORDER BY A",
    "SELECT A FROM T WHERE A > 5 AND A < 9 ORDER BY A",
    "SELECT B, COUNT(*) FROM T WHERE B >= 2 AND B <= 2 GROUP BY B",
];

/// Verifies: [EXTENSION] RULES.md D275, D276: an optimizer rule of an
/// extension rewrites the plan form, one call a pass; the plan it gives,
/// a filter no row passes made an empty VALUES, is planned and run, and
/// EXPLAIN names the rule among those that fired; each query returns the
/// rows it returns with no rule, the extension dropped.
#[test]
fn a_rule_rewrites_the_plan() {
    let (_scratch, db) = planner("plugin-planner-rule", "OPTIMIZER RULE CONTRADICTION PHASE BOUND;\n");
    let s = db.session("U");
    let with: Vec<Vec<String>> = QUERIES.iter().map(|q| rows(&s, q).unwrap()).collect();
    assert_eq!(with[0], Vec::<String>::new());
    assert_eq!(with[1], ["6", "7", "8"]);
    let shown = explained(&s, QUERIES[0]);
    assert!(shown.contains("PLANNER.CONTRADICTION"), "{shown}");
    assert!(!shown.contains("SeqScan"), "no table is read: {shown}");
    assert!(!explained(&s, QUERIES[1]).contains("PLANNER.CONTRADICTION"), "no change");
    let before = calls(&s);
    rows(&s, QUERIES[1]).unwrap();
    assert_eq!(calls(&s) - before, 2, "a call a statement planned, the count's own among them");
    s.commit().unwrap();
    s.run("DROP EXTENSION planner").unwrap();
    let without: Vec<Vec<String>> = QUERIES.iter().map(|q| rows(&s, q).unwrap()).collect();
    assert_eq!(with, without);
}

/// Verifies: [EXTENSION] RULES.md D277: a rule that gives a plan not
/// valid, one that loses a column, and a rule that fails are skipped for
/// the statement: it completes with its rows and the warning 01T02, and
/// EXPLAIN names each rule skipped.
#[test]
fn a_rule_that_fails_is_skipped() {
    let (_scratch, db) = planner("plugin-planner-skipped", "OPTIMIZER RULE BROKEN;\nOPTIMIZER RULE ERRING PASSES 3;\n");
    let s = db.session("U");
    assert_eq!(rows(&s, QUERIES[1]).unwrap(), ["6", "7", "8"]);
    assert_eq!(rows(&s, QUERIES[2]).unwrap(), ["2,5"]);
    let shown = explained(&s, QUERIES[1]);
    assert!(shown.contains("PLANNER.BROKEN skipped") && shown.contains("PLANNER.ERRING skipped"), "{shown}");
    let done = completed(&s, "SELECT COUNT(*) INTO N FROM T WHERE A > 5");
    assert_eq!(done.sqlstate().as_str(), "01T02", "{done:?}");
}

/// The objects of a test of statistics: FILES and SORTED, which answer
/// the planner's statistics, and SKEW, which asks the database's.
const STATISTICS: &str = "\
STORAGE ENGINE FILES;
INDEX METHOD SORTED SUPPORTS (EQUALITY, RANGE);
PROCEDURE SKEW(VARCHAR(128), VARCHAR(128)) MODIFIES SQL DATA;
";

/// Verifies: [EXTENSION] RULES.md D278: a plug-in engine's statistics,
/// asked once and kept, give the planner its estimates of a filter's
/// rows; asked again once the rows written reach the count the answer
/// said, and when the table is analyzed; an index method's distinct keys
/// give an equality's estimate through the index.
#[test]
fn an_engine_and_an_index_give_statistics() {
    let (_scratch, db) = planner("plugin-planner-to", STATISTICS);
    let s = db.session("U");
    s.run("CREATE TABLE F (K INTEGER, V INTEGER) USING FILES").unwrap();
    for v in 0..100 {
        s.run(&format!("INSERT INTO F VALUES ({}, {v})", v % 4)).unwrap();
    }
    s.commit().unwrap();
    let query = "SELECT V FROM F WHERE K = 1";
    let before = calls(&s);
    let shown = explained(&s, query);
    assert!(shown.contains("est. rows=25"), "100 rows of 4 keys: {shown}");
    let asked = calls(&s);
    assert!(asked > before, "the engine was asked");
    explained(&s, query);
    assert_eq!(calls(&s), asked, "its answer kept");
    db.analyze("U.F").unwrap();
    assert_eq!(calls(&s), asked + 1, "asked again as the table is analyzed");
    for v in 100..200 {
        s.run(&format!("INSERT INTO F VALUES ({}, {v})", v % 4)).unwrap();
    }
    s.commit().unwrap();
    let shown = explained(&s, query);
    assert!(shown.contains("est. rows=50"), "200 rows of 4 keys, asked again: {shown}");
    s.run("CREATE TABLE N (K INTEGER, V INTEGER)").unwrap();
    let values: Vec<String> = (0..300).map(|v| format!("({}, {v})", v % 100)).collect();
    s.run(&format!("INSERT INTO N VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    s.run("CREATE INDEX N_K ON N (K) USING SORTED").unwrap();
    s.commit().unwrap();
    let shown = explained(&s, "SELECT V FROM N WHERE K = 5");
    assert!(shown.contains("N_K") && shown.contains("est. rows=3"), "300 entries of 100 keys: {shown}");
    assert_eq!(rows(&s, "SELECT V FROM N WHERE K = 5 ORDER BY V").unwrap(), ["5", "105", "205"]);
}

/// Verifies: [EXTENSION] RULES.md D279: a procedure asks the database's
/// statistics of a column, by its user's privileges: of its own table
/// those ANALYZE gathered, of a table it may not read refused, as of a
/// table there is not; each refusal the procedure's error, SQLSTATE 38000.
#[test]
fn a_procedure_asks_the_database() {
    let (_scratch, db) = planner("plugin-planner-from", STATISTICS);
    db.schema("CREATE SCHEMA AUTHORIZATION W CREATE TABLE WT (X INTEGER)").unwrap();
    db.analyze("U.T").unwrap();
    let s = db.session("U");
    s.run("CREATE TABLE SEEN (N DOUBLE PRECISION, D DOUBLE PRECISION)").unwrap();
    s.run("CALL SKEW('U.T', 'B')").unwrap();
    assert_eq!(rows(&s, "SELECT CAST(N AS INTEGER), CAST(D AS INTEGER) FROM SEEN").unwrap(), ["20,4"]);
    let refused = s.run("CALL SKEW('W.WT', 'X')").unwrap_err();
    assert!(refused.msg.contains("no SELECT privilege on W.WT"), "U may not read W.WT: {refused:?}");
    let none = s.run("CALL SKEW('U.NONE', 'X')").unwrap_err();
    assert!(none.msg.contains("there is no table U.NONE"), "{none:?}");
}
