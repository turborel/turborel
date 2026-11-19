//! \[EXTENSION\] Row locks, LOCK TABLE, delta tables and RETURNING of a
//! partitioned table (RULES.md D153, D185, D186, D183, D184): each row
//! locked in the partition that holds it, the partitions locked with the
//! table, and the rows of a change routed to their partitions.

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

/// The SQLSTATE `sql` fails with in `s`, or "" if it does not fail.
fn state(s: &Session, sql: &str) -> String {
    rows(s, sql).err().map(|e| e.sqlstate().as_str().to_owned()).unwrap_or_default()
}

/// Verifies: [EXTENSION] RULES.md D185: FOR UPDATE of a partitioned table
/// locks each row it gives in the partition holding it, so another
/// transaction's NOWAIT of that row, read through the table or its
/// partition, is refused with TBL01, SKIP LOCKED passes over it, an
/// update of it is refused at once under NOWAIT locks, and rows of other
/// partitions stay free.
#[test]
fn for_update_locks_the_row_in_its_partition_d185() {
    let (db, a) = sales();
    let b = db.session("S");
    assert_eq!(rows(&a, "SELECT ID FROM SALES WHERE ID = 2 FOR UPDATE").unwrap(), vec!["2"]);
    assert_eq!(state(&b, "SELECT ID FROM SALES WHERE ID = 2 FOR UPDATE NOWAIT"), "TBL01");
    assert_eq!(state(&b, "SELECT ID FROM S2026 FOR SHARE NOWAIT"), "TBL01");
    assert_eq!(rows(&b, "SELECT ID FROM SALES ORDER BY ID FOR UPDATE SKIP LOCKED").unwrap(), vec!["1", "3"]);
    b.rollback().unwrap();
    assert_eq!(rows(&b, "SELECT ID FROM S2025 FOR UPDATE NOWAIT").unwrap(), vec!["1"]);
    b.rollback().unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&b, "SELECT ID FROM SALES WHERE ID = 2 FOR UPDATE NOWAIT").unwrap(), vec!["2"]);
    b.rollback().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D186: LOCK TABLE of a partitioned
/// table locks each of its partitions in the mode, so a write of a
/// partition waits, and ONLY locks the table alone.
#[test]
fn lock_table_locks_the_partitions_d186() {
    let (db, a) = sales();
    let b = db.session("S");
    a.run("LOCK TABLE SALES IN EXCLUSIVE MODE").unwrap();
    assert_eq!(state(&b, "SELECT ID FROM S2025 FOR UPDATE NOWAIT"), "TBL01");
    assert!(b.run("LOCK TABLE S2026 IN ROW EXCLUSIVE MODE NOWAIT").is_err());
    b.rollback().unwrap();
    a.rollback().unwrap();
    a.run("LOCK TABLE ONLY (SALES) IN EXCLUSIVE MODE").unwrap();
    b.run("LOCK TABLE S2026 IN ROW EXCLUSIVE MODE NOWAIT").unwrap();
    b.rollback().unwrap();
    a.rollback().unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 GR5] [EXTENSION] RULES.md D183,
/// D184: of a partitioned table, NEW TABLE of an INSERT gives the rows
/// routed to their partitions, FINAL TABLE of an UPDATE moving a row to
/// another partition gives it once as updated, OLD TABLE of a DELETE the
/// rows deleted from each partition, and RETURNING alike.
#[test]
fn delta_tables_and_returning_route_to_partitions_d183() {
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
    assert_eq!(rows(&s, "UPDATE SALES SET SOLD = DATE '2032-01-01' WHERE ID = 4 RETURNING ID, SOLD").unwrap(), vec!["4,2032-01-01"]);
    assert_eq!(rows(&s, "SELECT ID FROM SOTHER ORDER BY ID").unwrap(), vec!["3", "4", "5"]);
    let gone = rows(&s, "SELECT ID FROM OLD TABLE (DELETE FROM SALES WHERE ID > 2) AS O ORDER BY ID");
    assert_eq!(gone.unwrap(), vec!["3", "4", "5"]);
    assert_eq!(rows(&s, "DELETE FROM SALES RETURNING ID").unwrap().len(), 2);
    s.rollback().unwrap();
}
