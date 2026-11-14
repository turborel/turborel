//! Fixtures shared by modules: STAFF and DEPTS of the ORDER BY, OFFSET
//! and FETCH FIRST tests, the SQLSTATE a query raises, and T of prices of
//! the row pattern recognition tests.

use turborel::db::{Database, Session};

/// Schema A: STAFF of five rows, one of no department and no salary;
/// DEPTS of three departments, keyed.
pub fn staff() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A \
         CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(20), DEPT CHAR(3), SALARY INTEGER) \
         CREATE TABLE DEPTS (DEPT CHAR(3) PRIMARY KEY, TITLE VARCHAR(20))",
    )
    .unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO STAFF VALUES (1, 'Ada', 'ENG', 7200), (2, 'Grace', 'ENG', 8100), (3, 'Edsger', 'OPS', 5400), \
         (4, 'Barbara', NULL, NULL), (5, 'Alan', 'OPS', 6100)",
    )
    .unwrap();
    a.run("INSERT INTO DEPTS VALUES ('ENG', 'Engineering'), ('OPS', 'Operations'), ('HR', 'People')").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The SQLSTATE of the error `sql` raises as it opens or its rows are
/// read.
pub fn state(a: &Session, sql: &str) -> String {
    let read = a.open_cursor(sql).and_then(|mut c| {
        while c.fetch()?.is_some() {}
        Ok(())
    });
    read.err().map_or_else(|| panic!("no error: {sql}"), |e| e.sqlstate().as_str().to_owned())
}

/// Schema A, table T of prices by TICKER and day D. Of A, days 1 to 9 at
/// 10, 9, 8, 9, 10, 11, 10, 9, 12: a V of days 1 to 6, then one of 7 to 9.
/// Of B, days 1 to 3 at 5, 4, 5.
pub fn prices() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (TICKER CHARACTER(1) NOT NULL, D INTEGER NOT NULL, PRICE INTEGER, PRIMARY KEY (TICKER, D))")
        .unwrap();
    let a = db.session("A");
    let rows = "('A', 1, 10), ('A', 2, 9), ('A', 3, 8), ('A', 4, 9), ('A', 5, 10), ('A', 6, 11), ('A', 7, 10), ('A', 8, 9), ('A', 9, 12), ('B', 1, 5), ('B', 2, 4), ('B', 3, 5)";
    a.run(&format!("INSERT INTO T VALUES {rows}")).unwrap();
    a.commit().unwrap();
    (db, a)
}

/// MATCH_RECOGNIZE over T partitioned by TICKER and ordered by D, of
/// the measures `measures`, the rows per match `rows`, the skip `skip`
/// and the pattern `pattern`, DOWN a price below the last and UP above.
pub fn recognized(measures: &str, rows: &str, skip: &str, pattern: &str) -> String {
    format!(
        "SELECT * FROM T MATCH_RECOGNIZE (PARTITION BY TICKER ORDER BY D MEASURES {measures} {rows} AFTER MATCH {skip} PATTERN ({pattern}) \
         DEFINE DOWN AS PRICE < PREV(PRICE), UP AS PRICE > PREV(PRICE)) AS R"
    )
}
