//! \[DESIGN\] Access paths no estimate can make a scan (RULES.md D839). A
//! whole unique key held equal is probed, whatever the table's size and
//! statistics, in a query, a prepared statement and a procedure kept before
//! the table's rows came, and a plan kept before its tables had statistics
//! is learned again once they do. Every database here gathers statistics
//! only when a test asks (JOBS.POLICY MANUAL), so each test says which it
//! plans with, none, stale or misleading.

use turborel::db::{Database, Prepared, Session};
use turborel::params::Params;
use turborel::physical::plannings;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::rows;

/// A database of the table G, keyed by (G, K), its statistics gathered
/// only when asked, and a session of its schema.
fn keyed() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE G (G INTEGER NOT NULL, K INTEGER NOT NULL, V INTEGER, PRIMARY KEY (G, K))")
        .unwrap();
    let s = db.session("X");
    (db, s)
}

/// Rows of G: groups `groups`, keys 1 to `per` of each, each V its key.
fn loaded(s: &Session, groups: std::ops::RangeInclusive<i64>, per: i64) {
    let values: Vec<String> = groups.flat_map(|g| (1..=per).map(move |k| format!("({g}, {k}, {k})"))).collect();
    for part in values.chunks(200) {
        s.run(&format!("INSERT INTO G VALUES {}", part.join(", "))).unwrap();
    }
    s.commit().unwrap();
}

/// The physical plan of `sql` in `s`.
fn plan(s: &Session, sql: &str) -> String {
    let shown = s.explain(sql, &Params::new()).unwrap();
    s.rollback().unwrap();
    shown.split("-- physical:").nth(1).unwrap_or(&shown).to_owned()
}

/// The rows the statements of `s`'s user whose text begins `text` read.
fn read(s: &Session, text: &str) -> u64 {
    let sql = format!("SELECT ROWS_READ FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS WHERE STATEMENT_TEXT LIKE '{text}%'");
    let found = rows(s, &sql).unwrap();
    s.commit().unwrap();
    found.first().map_or(0, |n| n.trim().parse().unwrap())
}

fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The key probed by each of the queries over G that hold it whole.
const POINTS: [&str; 3] = [
    "SELECT V FROM G WHERE G = 2 AND K = 3",
    "SELECT V FROM G WHERE K = 3 AND G = 2 ORDER BY V DESC",
    "SELECT V FROM G WHERE G = 2 AND K = 3 AND V > 0",
];

/// Whether `shown` reads G by its key's probe, with no scan nor sort.
fn probed(shown: &str) -> bool {
    shown.contains("IndexScan") && !shown.contains("SeqScan") && !shown.contains("Sort")
}

/// Verifies: [DESIGN] RULES.md D839: a query holding every column of a
/// primary key equal reads by the key's probe, with no sort of the one row
/// it finds, of a table empty, of one of a few rows, where a scan is priced
/// below a probe, and of one whose statistics, gathered when it held one
/// group, say every row is of the group asked.
///
/// Regression test (R-TEST-7): a table of fewer than a thousand rows was
/// scanned for its key, as DISTRICT and WAREHOUSE were.
#[test]
fn a_whole_unique_key_is_probed_whatever_the_statistics() {
    let (db, s) = keyed();
    for sql in POINTS {
        assert!(probed(&plan(&s, sql)), "empty: {sql}: {}", plan(&s, sql));
    }
    loaded(&s, 2..=2, 10);
    for sql in POINTS {
        assert!(probed(&plan(&s, sql)), "ten rows: {sql}: {}", plan(&s, sql));
    }
    db.analyze("X.G").unwrap();
    loaded(&s, 3..=40, 10);
    for sql in POINTS {
        assert!(probed(&plan(&s, sql)), "statistics of one group: {sql}: {}", plan(&s, sql));
    }
    assert_eq!(rows(&s, POINTS[1]), Ok(vec!["3".to_owned()]));
}

/// Verifies: [DESIGN] RULES.md D839, D45, D414: a prepared query and a
/// procedure's statements reading and changing a row by its whole key,
/// first run while the table was empty, read the row alone at each later
/// execution, of a table of a few rows and of one of many.
///
/// Regression test (R-TEST-7): the plans learned of the empty table were
/// scans, kept until the table grew past them.
#[test]
fn a_whole_key_kept_before_the_load_reads_one_row() {
    let (_db, s) = keyed();
    s.run(
        "CREATE PROCEDURE BUMP (IN A INTEGER, IN B INTEGER) MODIFIES SQL DATA BEGIN
           DECLARE W INTEGER;
           SELECT V INTO W FROM G WHERE G = A AND K = B;
           UPDATE G SET V = V + 1 WHERE G = A AND K = B;
         END",
    )
    .unwrap();
    s.commit().unwrap();
    let query = s.prepare("SELECT V FROM G WHERE G = ? AND K = ?").unwrap();
    let call = s.prepare("CALL BUMP(?, ?)").unwrap();
    let queries = |n: i64| {
        for k in 1..=n {
            let mut cursor = s.open_prepared(&query, &[int(2), int(k)]).unwrap();
            while cursor.fetch().unwrap().is_some() {}
            s.commit().unwrap();
        }
    };
    let calls = |n: i64| {
        for k in 1..=n {
            s.execute_prepared(&call, &[int(2), int(k)]).unwrap();
            s.commit().unwrap();
        }
    };
    queries(8);
    calls(8);
    for (groups, per) in [(2..=2, 12), (3..=100, 12)] {
        loaded(&s, groups, per);
        let (query_before, call_before) = (read(&s, "SELECT V FROM G"), read(&s, "CALL BUMP"));
        queries(10);
        calls(10);
        let (query_read, call_read) = (read(&s, "SELECT V FROM G") - query_before, read(&s, "CALL BUMP") - call_before);
        assert!(query_read <= 20 && call_read <= 40, "{per} a group: query {query_read}, call {call_read}");
    }
}

/// The plannings `n` executions of the prepared `p`, of V = 1 to `n`, made.
fn plannings_of(s: &Session, p: &Prepared, n: i64) -> u64 {
    let before = plannings();
    for v in 1..=n {
        let mut cursor = s.open_prepared(p, &[int(v)]).unwrap();
        while cursor.fetch().unwrap().is_some() {}
    }
    s.commit().unwrap();
    plannings().wrapping_sub(before)
}

/// Verifies: [DESIGN] RULES.md D839, D45: a prepared query whose plan
/// scans, learned before its table had statistics, is learned again once
/// they are gathered, though its table did not drift, and kept again
/// after; gathered again with no row changed, it is kept.
///
/// Regression test (R-TEST-7): only a plan of index probes alone was
/// learned again with statistics; one with a scan kept the plan made of
/// the defaults until its table drifted.
#[test]
fn a_scan_learned_before_statistics_is_learned_again_once_they_are() {
    let (db, s) = keyed();
    loaded(&s, 1..=50, 4);
    let p = s.prepare("SELECT G, K FROM G WHERE V = ?").unwrap();
    assert!(plannings_of(&s, &p, 40) > 0, "learned without statistics");
    assert_eq!(plannings_of(&s, &p, 40), 0, "kept");
    db.analyze("X.G").unwrap();
    assert!(plannings_of(&s, &p, 40) > 0, "learned again with them");
    assert_eq!(plannings_of(&s, &p, 40), 0, "kept again");
    db.analyze("X.G").unwrap();
    assert_eq!(plannings_of(&s, &p, 40), 0, "statistics gathered again of the same rows");
}

/// Verifies: [DESIGN] RULES.md D839, D414: a procedure's query whose plan
/// scans, learned before its table had statistics, is learned again once
/// they are gathered, and kept again after.
///
/// Regression test (R-TEST-7): as of a prepared query.
#[test]
fn a_procedures_scan_learned_before_statistics_is_learned_again() {
    let (db, s) = keyed();
    loaded(&s, 1..=50, 4);
    s.run(
        "CREATE PROCEDURE FIND (IN W INTEGER) READS SQL DATA BEGIN
           DECLARE N INTEGER;
           SELECT COUNT(*) INTO N FROM G WHERE V = W;
         END",
    )
    .unwrap();
    s.commit().unwrap();
    let call = s.prepare("CALL FIND(?)").unwrap();
    let calls = |n: i64| {
        let before = plannings();
        for v in 1..=n {
            s.execute_prepared(&call, &[int(v)]).unwrap();
        }
        s.commit().unwrap();
        plannings().wrapping_sub(before)
    };
    assert!(calls(40) > 0, "learned without statistics");
    assert_eq!(calls(40), 0, "kept");
    db.analyze("X.G").unwrap();
    assert!(calls(40) > 0, "learned again with them");
    assert_eq!(calls(40), 0, "kept again");
}
