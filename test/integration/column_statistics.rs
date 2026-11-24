//! INFORMATION_SCHEMA.COLUMN_STATISTICS (RULES.md D990, D991): what the
//! planner's statistics hold of each column, after ANALYZE and after the
//! statistics job, of several types, of an empty table and of a column all
//! NULL; when they were gathered; who sees which column; and the distinct
//! keys INDEX_STATISTICS shows of an index (D992).

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::Database;
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::rows;

/// The instant the clock stands at: 2024-02-29 12:34:56.789012 UTC.
const NOW: i64 = 1_709_210_096_789_012;

/// What a column's row shows past its name.
const FIGURES: &str = "ROW_COUNT, GATHERED, NULL_FRACTION, DISTINCT_VALUES, MIN_VALUE, MAX_VALUE, \
                       MOST_COMMON_VALUES, MOST_COMMON_FREQUENCIES, HISTOGRAM_BOUNDS";

/// A database whose owner is OWNER, its clock at `NOW`, its maintenance
/// run only as a test asks, with X's table S of 100 rows: I from 0 to 99,
/// C 'a' in 60 of them, 'b' in 30 and 'c' in 10, D one of two dates, N
/// always NULL, and an index on I; and X's table E, empty.
fn owned() -> (Database, Arc<FixedClock>) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let clock = Arc::new(FixedClock::at(NOW));
    db.set_clock(Arc::clone(&clock) as Arc<dyn turborel::clock::Clock>).unwrap();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION X \
         CREATE TABLE S (I INTEGER, C CHARACTER VARYING(10), D DATE, N INTEGER) \
         CREATE TABLE E (K INTEGER, V CHARACTER VARYING(10))",
    )
    .unwrap();
    let x = db.session("X");
    x.run("CREATE INDEX S_I ON S (I)").unwrap();
    let c = |k: u32| {
        if k < 60 {
            "a"
        } else if k < 90 {
            "b"
        } else {
            "c"
        }
    };
    let d = |k: u32| if k.is_multiple_of(2) { "2024-01-01" } else { "2024-12-31" };
    let values: Vec<String> = (0..100).map(|k| format!("({k}, '{}', DATE '{}', NULL)", c(k), d(k))).collect();
    x.run(&format!("INSERT INTO S VALUES {}", values.join(", "))).unwrap();
    x.commit().unwrap();
    (db, clock)
}

/// The rows of COLUMN_STATISTICS of `table`, as OWNER sees them: each
/// column's name and `FIGURES`.
fn figures(db: &Database, table: &str) -> Vec<String> {
    let sql = format!(
        "SELECT COLUMN_NAME, {FIGURES} FROM INFORMATION_SCHEMA.COLUMN_STATISTICS \
         WHERE TABLE_SCHEMA = 'X' AND TABLE_NAME = '{table}' ORDER BY ORDINAL_POSITION"
    );
    rows(&db.session("OWNER"), &sql).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1]; [DESIGN] RULES.md
/// D990, D991: after ANALYZE, each column's rows, when they were gathered,
/// its share NULL, its distinct values, its least and greatest values as
/// text, its most common values with their shares, and its
/// histogram's bounds, of an integer, a character string and a date
/// column, and of one always NULL.
#[test]
fn each_columns_statistics_are_shown_after_analyze() {
    let (db, _clock) = owned();
    db.analyze("X.S").unwrap();
    let shown = figures(&db, "S");
    assert_eq!(shown.len(), 4, "a row of each column: {shown:?}");
    let at = "2024-02-29 12:34:56.789012+00:00";
    assert!(shown[0].starts_with(&format!("I,100,{at},0.0000,100,0,99,")), "{}", shown[0]);
    assert!(shown[1].starts_with(&format!("C,100,{at},0.0000,3,a,c,ARRAY['a'],ARRAY[0.6000],ARRAY['b', ")), "{}", shown[1]);
    assert!(shown[1].ends_with("'c']"), "the rest's bounds, 'b' and 'c' alone: {}", shown[1]);
    assert!(shown[2].starts_with(&format!("D,100,{at},0.0000,2,2024-01-01,2024-12-31,")), "{}", shown[2]);
    let back = "SELECT CAST(MAX_VALUE AS DATE) FROM INFORMATION_SCHEMA.COLUMN_STATISTICS WHERE COLUMN_NAME = 'D'";
    assert_eq!(rows(&db.session("OWNER"), back).unwrap(), ["2024-12-31"], "a CAST reads the value back");
    assert_eq!(shown[3], format!("N,100,{at},1.0000,0,NULL,NULL,ARRAY[],ARRAY[],ARRAY[]"), "always NULL");
    let bounds = "SELECT CARDINALITY(HISTOGRAM_BOUNDS) > 2 FROM INFORMATION_SCHEMA.COLUMN_STATISTICS WHERE COLUMN_NAME = 'I'";
    assert_eq!(rows(&db.session("OWNER"), bounds).unwrap(), ["TRUE"], "I's histogram has bounds");
    let width = "SELECT AVERAGE_WIDTH > 0 FROM INFORMATION_SCHEMA.COLUMN_STATISTICS WHERE COLUMN_NAME = 'C'";
    assert_eq!(rows(&db.session("OWNER"), width).unwrap(), ["TRUE"], "a row read for C has a width");
}

/// Verifies: [DESIGN] RULES.md D990: an empty table's statistics, once
/// gathered, are of no rows, no value of any column known, and its arrays
/// empty.
#[test]
fn an_empty_tables_statistics_are_of_no_rows() {
    let (db, _clock) = owned();
    db.analyze("X.E").unwrap();
    let at = "2024-02-29 12:34:56.789012+00:00";
    let none = |c: &str| format!("{c},0,{at},0.0000,0,NULL,NULL,ARRAY[],ARRAY[],ARRAY[]");
    assert_eq!(figures(&db, "E"), [none("K"), none("V")]);
}

/// Verifies: [DESIGN] RULES.md D990, D991: statistics not gathered show
/// NULL; reading the view makes them due, as a plan's reading them does,
/// and the statistics job then gathers them and stamps when, its clock's
/// instant; a table emptied by TRUNCATE has none again, and no stamp
/// shown from the storage it replaced.
#[test]
fn statistics_gathered_by_the_job_are_stamped_when() {
    let (db, clock) = owned();
    let none = |c: &str| format!("{c},NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL");
    assert_eq!(figures(&db, "E"), [none("K"), none("V")], "not gathered yet");
    clock.set(NOW + 60_000_000);
    db.maintain().unwrap();
    let gathered = "SELECT DISTINCT ROW_COUNT, GATHERED FROM INFORMATION_SCHEMA.COLUMN_STATISTICS WHERE TABLE_NAME = 'S'";
    assert_eq!(rows(&db.session("OWNER"), gathered).unwrap(), ["100,2024-02-29 12:35:56.789012+00:00"], "the job's pass");
    let x = db.session("X");
    x.run("TRUNCATE TABLE S").unwrap();
    x.commit().unwrap();
    assert_eq!(rows(&db.session("OWNER"), gathered).unwrap(), ["NULL,NULL"], "a new storage, none gathered");
}

/// Verifies: [DESIGN] RULES.md D520, D990: the owner sees every column's
/// row; another user those of the columns on which it holds a privilege,
/// SELECT on the table being on each; one granted the owner's role
/// MONITOR every one. Pairs: (owner, privilege, role) = (yes, -, -), (no,
/// none, none), (no, UPDATE of C, none), (no, SELECT, none), (no, none,
/// MONITOR).
#[test]
fn a_user_sees_the_columns_it_holds_a_privilege_on() {
    let (db, _clock) = owned();
    db.create_user("Y", b"x").unwrap();
    db.create_user("Z", b"x").unwrap();
    let sql = "SELECT TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMN_STATISTICS WHERE TABLE_SCHEMA = 'X' \
               ORDER BY TABLE_NAME, ORDINAL_POSITION";
    assert_eq!(rows(&db.session("OWNER"), sql).unwrap(), ["E,K", "E,V", "S,I", "S,C", "S,D", "S,N"]);
    let y = db.session("Y");
    assert_eq!(rows(&y, sql).unwrap(), Vec::<String>::new(), "no privilege");
    let x = db.session("X");
    x.run("GRANT UPDATE (C) ON S TO Y").unwrap();
    x.run("GRANT SELECT ON E TO Y").unwrap();
    x.run("GRANT DELETE ON S TO Z").unwrap();
    x.commit().unwrap();
    assert_eq!(rows(&y, sql).unwrap(), ["E,K", "E,V", "S,C"], "SELECT on E's columns, UPDATE on C alone");
    assert_eq!(rows(&db.session("Z"), sql).unwrap(), Vec::<String>::new(), "DELETE is on no column");
    let owner = db.session("OWNER");
    owner.run("CREATE ROLE MONITOR").unwrap();
    owner.run("GRANT MONITOR TO Y").unwrap();
    owner.commit().unwrap();
    assert_eq!(rows(&y, sql).unwrap().len(), 6, "MONITOR shows them all");
}

/// Verifies: [ISO/IEC 9075:1992 §21.1]; [DESIGN] RULES.md D990: the view
/// cannot be written: a DELETE from it, by the database's owner, is
/// refused for want of the privilege (SQLSTATE 42000, SQLCODE -104).
#[test]
fn the_view_cannot_be_written() {
    let (db, _clock) = owned();
    let err = db.session("OWNER").run("DELETE FROM INFORMATION_SCHEMA.COLUMN_STATISTICS").unwrap_err();
    assert_eq!((err.code, err.sqlstate().as_str(), err.sqlcode()), (Code::Privilege, "42000", -104), "{err}");
}

/// Verifies: [DESIGN] RULES.md D992: INDEX_STATISTICS shows the distinct
/// keys an index counts, as the planner reads them: near 100 of S_I's
/// 100 keys.
#[test]
fn an_indexs_distinct_keys_are_shown() {
    let (db, _clock) = owned();
    let owner = db.session("OWNER");
    let sql = "SELECT DISTINCT_KEYS BETWEEN 95 AND 105 FROM INFORMATION_SCHEMA.INDEX_STATISTICS WHERE INDEX_NAME = 'S_I'";
    assert_eq!(rows(&owner, sql).unwrap(), ["TRUE"]);
}
