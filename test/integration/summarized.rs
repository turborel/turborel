//! \[DESIGN\] MIN and MAX of a table of the column store answered by its
//! groups' summaries (RULES.md D1181): the answer a scan gives, row by row
//! and of the row store alike, as frozen rows holding a least or greatest
//! value are deleted and changed, rows are added past the groups, and
//! another session's changes are not yet committed; noted by EXPLAIN, and
//! left to the scan of a text, a filter or a set function of another kind.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{both_ways, exec, rows};

/// Set functions summaries answer, with a COUNT(*) among them.
const BOUNDED: &str = "SELECT MIN(K), MAX(K), COUNT(*), MIN(N), MAX(N), MIN(D), MAX(D) FROM {t}";

/// Tables R, of the row store, and C, of the column store in groups of
/// 2048 sorted by G, of schema A, alike: 5000 rows, N null for every 9th,
/// frozen in C.
fn tables(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER, G INTEGER, N INTEGER, D DATE, S VARCHAR(20))
           CREATE TABLE C (K INTEGER, G INTEGER, N INTEGER, D DATE, S VARCHAR(20))
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 'G')",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..5000).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|k: i64| {
                    let n = if k % 9 == 0 { "NULL".to_owned() } else { (k % 1000).to_string() };
                    format!("({k}, {}, {n}, DATE '2013-07-01' + INTERVAL '{}' DAY, 's{}')", k % 7, k % 30, k % 13)
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    a
}

/// What `s` reads of `query` of R and of C, and C's read row by row.
fn of_each(s: &Session, query: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (batched, row) = both_ways(s, &query.replace("{t}", "C"));
    (rows(s, &query.replace("{t}", "R")).unwrap(), batched, row)
}

/// Asserts C answers `query` as R does and as its scan row by row does.
fn alike(s: &Session, query: &str, why: &str) {
    let (row_store, batched, row) = of_each(s, query);
    assert_eq!(batched, row_store, "{why}: {query}");
    assert_eq!(batched, row, "{why}: {query}");
}

/// Verifies: [X3.135-1986 §5.8 GR1, GR4b] [DESIGN] RULES.md D1181, D29:
/// MIN and MAX of the column store, by its summaries, give what the row
/// store and a scan row by row give: of frozen rows; once the least and
/// greatest K are deleted and a frozen row changed to a new greatest; of
/// rows added past the groups; and to another session, of the first's
/// changes not yet committed, then committed.
#[test]
fn min_and_max_by_summaries_are_the_scans() {
    let db = Database::new();
    let a = tables(&db);
    alike(&a, BOUNDED, "frozen");
    let other = db.session("A");
    for table in ["R", "C"] {
        exec(&a, &format!("DELETE FROM {table} WHERE K = 0 OR K = 4999")).unwrap();
        exec(&a, &format!("UPDATE {table} SET K = 9000, N = -5 WHERE K = 1234")).unwrap();
        exec(&a, &format!("INSERT INTO {table} VALUES (-7, 1, 2000, DATE '2014-01-01', 'new')")).unwrap();
    }
    alike(&a, BOUNDED, "its own changes");
    alike(&other, BOUNDED, "another's uncommitted changes");
    assert_eq!(rows(&other, &BOUNDED.replace("{t}", "C")).unwrap(), vec!["0,4999,5000,0,999,2013-07-01,2013-07-30".to_owned()]);
    a.commit().unwrap();
    alike(&other, BOUNDED, "committed");
    assert_eq!(rows(&other, &BOUNDED.replace("{t}", "C")).unwrap(), vec!["-7,9000,4999,-5,2000,2013-07-01,2014-01-01".to_owned()]);
    db.maintain().unwrap();
    alike(&other, BOUNDED, "maintained");
}

/// Verifies: [EXTENSION] [DESIGN] RULES.md D1181: EXPLAIN notes the table
/// whose summaries answer MIN and MAX, never split in parallel; of a text
/// column, a filter, a grouping, or a SUM beside them, it notes none, and
/// those still answer as a scan does.
#[test]
fn explain_notes_min_and_max_by_summaries() {
    let db = Database::new();
    let a = tables(&db);
    let plan = |sql: &str| a.explain(&sql.replace("{t}", "C"), &Params::new()).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    a.run("SET EXECUTOR.PARALLEL_MIN_ROWS = 1").unwrap();
    a.run("SET EXECUTOR.MORSEL_ROWS = 100").unwrap();
    assert!(plan("SELECT SUM(K) FROM {t}").contains("Parallel"), "a sum is split");
    let noted = plan(BOUNDED);
    assert!(noted.contains("-- summarized by storage: A.C") && !noted.contains("Parallel"), "{noted}");
    assert!(plan("SELECT MIN(K) FROM {t}").contains("-- summarized by storage: A.C"));
    for query in [
        "SELECT MIN(S), MAX(K) FROM {t}",
        "SELECT MIN(K) FROM {t} WHERE G = 3",
        "SELECT G, MIN(K) FROM {t} GROUP BY G",
        "SELECT MIN(K), SUM(N) FROM {t}",
        "SELECT COUNT(N), MAX(K) FROM {t}",
    ] {
        assert!(!plan(query).contains("summarized by storage"), "{query}");
        alike(&a, query, "left to the scan");
    }
    assert!(!plan("SELECT COUNT(*) FROM {t}").contains("summarized by storage"), "counted instead");
}
