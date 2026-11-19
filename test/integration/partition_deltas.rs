//! [ISO/IEC 9075-2:2023 §7.6] \[EXTENSION\] Data change delta tables of a
//! partitioned table (RULES.md D153, D174), the rows of a change routed to
//! its partitions.

use turborel::db::{Database, Session};
use turborel::error::SqlError;

/// A database whose schema S owns SALES, partitioned by RANGE of its
/// SOLD, of S2025, S2026 and a DEFAULT, holding one row in each,
/// committed; and the session of S.
fn sales() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    for sql in [
        "CREATE TABLE SALES (ID INTEGER NOT NULL, SOLD DATE NOT NULL, AMOUNT INTEGER) PARTITION BY RANGE (SOLD)",
        "CREATE TABLE S2025 PARTITION OF SALES FOR VALUES FROM (DATE '2025-01-01') TO (DATE '2026-01-01')",
        "CREATE TABLE S2026 PARTITION OF SALES FOR VALUES FROM (DATE '2026-01-01') TO (DATE '2027-01-01')",
        "CREATE TABLE SOTHER PARTITION OF SALES DEFAULT",
        "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10), (2, DATE '2026-03-01', 20), (3, DATE '2030-01-01', 30)",
    ] {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
    (db, s)
}

/// The rows `sql` gives in `s`, each its values joined by commas.
fn rows(s: &Session, sql: &str) -> Result<Vec<String>, SqlError> {
    let mut c = s.open_cursor(sql)?;
    let mut out = Vec::new();
    while let Some(r) = c.fetch()? {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 GR5] [EXTENSION] RULES.md D153,
/// D174: of a partitioned table, NEW TABLE of an INSERT gives the rows
/// routed to their partitions, FINAL TABLE of an UPDATE moving a row to
/// another partition gives it once as updated, OLD TABLE of a DELETE the
/// rows deleted from each partition.
#[test]
fn delta_tables_route_to_partitions_d174() {
    let (_db, s) = sales();
    let new = rows(
        &s,
        "SELECT ID FROM NEW TABLE (INSERT INTO SALES VALUES (4, DATE '2025-06-01', 40), (5, DATE '2031-01-01', 50)) AS N ORDER BY ID",
    );
    assert_eq!(new.unwrap(), vec!["4", "5"]);
    assert_eq!(rows(&s, "SELECT ID FROM S2025 ORDER BY ID").unwrap(), vec!["1", "4"]);
    let moved = rows(&s, "SELECT ID, AMOUNT FROM FINAL TABLE (UPDATE SALES SET SOLD = DATE '2026-06-01', AMOUNT = 11 WHERE ID = 1) AS F");
    assert_eq!(moved.unwrap(), vec!["1,11"]);
    assert_eq!(rows(&s, "SELECT ID FROM S2026 ORDER BY ID").unwrap(), vec!["1", "2"]);
    assert_eq!(rows(&s, "SELECT ID FROM S2025").unwrap(), vec!["4"]);
    let old = rows(&s, "SELECT ID, AMOUNT FROM OLD TABLE (UPDATE SALES SET SOLD = DATE '2025-01-02' WHERE ID = 2) AS O");
    assert_eq!(old.unwrap(), vec!["2,20"]);
    let far = rows(&s, "SELECT ID, SOLD FROM NEW TABLE (UPDATE SALES SET SOLD = DATE '2032-01-01' WHERE ID = 4) AS M");
    assert_eq!(far.unwrap(), vec!["4,2032-01-01"]);
    assert_eq!(rows(&s, "SELECT ID FROM SOTHER ORDER BY ID").unwrap(), vec!["3", "4", "5"]);
    let gone = rows(&s, "SELECT ID FROM OLD TABLE (DELETE FROM SALES WHERE ID > 2) AS O ORDER BY ID");
    assert_eq!(gone.unwrap(), vec!["3", "4", "5"]);
    assert_eq!(rows(&s, "SELECT ID FROM OLD TABLE (DELETE FROM SALES) AS O").unwrap().len(), 2);
    s.rollback().unwrap();
}
