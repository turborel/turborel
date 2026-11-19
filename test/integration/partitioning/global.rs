//! Global indexes: one index over every partition's rows, of a key that
//! lacks the partition key.

use turborel::error::Code;

use crate::common::{Scratch, exec, sorted};

use super::recovery::opened;
use super::{plan, sales};

/// The global index of SALES's constraint ONE_ID.
const ONE_ID: &str = "S.SALES.ONE_ID\u{0}\u{0}GLOBAL";

/// Verifies: [EXTENSION] RULES.md D157: a UNIQUE constraint without the
/// partition key is held by one global index over the partitions' rows,
/// each row's entry made as it is written, the key refused in any other
/// partition, SQLCODE -201; an entry taken back with its statement rolled
/// back; a row moved keeps its key.
#[test]
fn a_global_index_holds_a_key_across_partitions() {
    let (db, s) = sales();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10), (2, DATE '2026-03-01', 20)").unwrap();
    exec(&s, "ALTER TABLE SALES ADD CONSTRAINT ONE_ID UNIQUE (ID)").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(db.index_entries(ONE_ID).unwrap(), 2);
    assert_eq!(exec(&s, "INSERT INTO SALES VALUES (1, DATE '2030-03-01', 11)"), Err(Code::Constraint));
    assert_eq!(exec(&s, "INSERT INTO SALES VALUES (2, DATE '2025-03-01', 12)"), Err(Code::Constraint));
    exec(&s, "INSERT INTO SALES VALUES (3, DATE '2030-03-01', 30)").unwrap();
    exec(&s, "ROLLBACK").unwrap();
    assert_eq!(db.index_entries(ONE_ID).unwrap(), 2);
    exec(&s, "UPDATE SALES SET SOLD = DATE '2026-05-01' WHERE ID = 1").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM S2026"), ["1", "2"]);
    assert_eq!(exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 13)"), Err(Code::Constraint));
    exec(&s, "DELETE FROM SALES WHERE ID = 1").unwrap();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 14)").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT AMOUNT FROM SALES WHERE ID = 1"), ["14.00"]);
}

/// Verifies: [EXTENSION] RULES.md D157: a read fixing the key of a global
/// index probes it, once, not each partition, and finds the row in
/// whichever partition holds it; a read that does not fix it reads the
/// partitions.
#[test]
fn a_global_index_is_read() {
    let (_db, s) = sales();
    exec(&s, "ALTER TABLE SALES ADD CONSTRAINT ONE_ID UNIQUE (ID)").unwrap();
    for i in 0..300 {
        let year = 2025 + i % 3;
        exec(&s, &format!("INSERT INTO SALES VALUES ({i}, DATE '{year}-02-01', {i})")).unwrap();
    }
    exec(&s, "COMMIT").unwrap();
    let shown = plan(&s, "SELECT AMOUNT FROM SALES WHERE ID = 7");
    assert!(shown.contains("GLOBAL") && !shown.contains("Append"), "{shown}");
    assert_eq!(sorted(&s, "SELECT AMOUNT FROM SALES WHERE ID = 7"), ["7.00"]);
    assert_eq!(sorted(&s, "SELECT SOLD FROM SALES WHERE ID = 299"), ["2027-02-01"]);
    assert_eq!(sorted(&s, "SELECT SOLD FROM SALES WHERE ID = 300"), Vec::<String>::new());
    let shown = plan(&s, "SELECT ID FROM SALES WHERE AMOUNT = 7");
    assert!(!shown.contains("GLOBAL"), "{shown}");
}

/// Verifies: [EXTENSION] RULES.md D157: ATTACH adds the rows attached to
/// the global index before they are checked, so a key another partition
/// holds is refused, SQLCODE -201, and the index left as it was; DETACH
/// takes its rows out, so the key may be written again; TRUNCATE of a
/// partition takes its rows out too; DROP INDEX refuses the global index.
#[test]
fn attach_and_detach_change_a_global_index() {
    let (db, s) = sales();
    exec(&s, "ALTER TABLE SALES ADD CONSTRAINT ONE_ID UNIQUE (ID)").unwrap();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10)").unwrap();
    exec(&s, "CREATE TABLE S2027 (ID INTEGER NOT NULL, SOLD DATE NOT NULL, AMOUNT DECIMAL(9,2))").unwrap();
    exec(&s, "INSERT INTO S2027 VALUES (1, DATE '2027-03-01', 11), (2, DATE '2027-04-01', 12)").unwrap();
    exec(&s, "COMMIT").unwrap();
    let attach = "ALTER TABLE SALES ATTACH PARTITION S2027 FOR VALUES FROM (DATE '2027-01-01') TO (DATE '2028-01-01')";
    assert_eq!(exec(&s, attach), Err(Code::Constraint));
    assert_eq!(db.index_entries(ONE_ID).unwrap(), 1);
    exec(&s, "DELETE FROM S2027 WHERE ID = 1").unwrap();
    exec(&s, "COMMIT").unwrap();
    exec(&s, attach).unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(exec(&s, "INSERT INTO SALES VALUES (2, DATE '2025-05-01', 13)"), Err(Code::Constraint));
    exec(&s, "ALTER TABLE SALES DETACH PARTITION S2027").unwrap();
    exec(&s, "COMMIT").unwrap();
    exec(&s, "INSERT INTO SALES VALUES (2, DATE '2025-05-01', 13)").unwrap();
    exec(&s, "COMMIT").unwrap();
    exec(&s, "TRUNCATE TABLE S2025").unwrap();
    exec(&s, "COMMIT").unwrap();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2026-05-01', 14), (2, DATE '2026-05-01', 15)").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM SALES"), ["1", "2"]);
    assert!(exec(&s, &format!("DROP INDEX \"{ONE_ID}\"")).is_err());
}

/// Verifies: [EXTENSION] RULES.md D157: a constraint holding the partition
/// key needs no global index; one dropped takes its global index with it.
#[test]
fn a_global_index_is_made_only_when_needed() {
    let (db, s) = sales();
    assert!(db.index_entries("S.SALES.SALES_PRIMARY1\u{0}\u{0}GLOBAL").is_err());
    exec(&s, "ALTER TABLE SALES ADD CONSTRAINT ONE_ID UNIQUE (ID)").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert!(db.index_entries(ONE_ID).is_ok());
    exec(&s, "ALTER TABLE SALES DROP CONSTRAINT ONE_ID CASCADE").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert!(db.index_entries(ONE_ID).is_err());
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10), (1, DATE '2026-03-01', 11)").unwrap();
}

/// Verifies: [EXTENSION] RULES.md D157: a global index is the same after
/// recovery from the log alone and after a checkpoint: of the rows written
/// and attached, not those detached, and still holding its key.
#[test]
fn a_global_index_is_recovered() {
    let scratch = Scratch::new("partitioning-global");
    let dir = scratch.path("db");
    let db = opened(&dir, true);
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    for sql in [
        "CREATE TABLE T (K INTEGER NOT NULL, V INTEGER NOT NULL, CONSTRAINT ONE_V UNIQUE (V)) PARTITION BY RANGE (K)",
        "CREATE TABLE T0 PARTITION OF T FOR VALUES FROM (0) TO (100)",
        "CREATE TABLE T1 PARTITION OF T FOR VALUES FROM (100) TO (200)",
        "CREATE TABLE LOOSE (K INTEGER NOT NULL, V INTEGER NOT NULL)",
        "INSERT INTO T VALUES (1, 1), (150, 2)",
        "INSERT INTO LOOSE VALUES (250, 3)",
        "COMMIT",
        "ALTER TABLE T ATTACH PARTITION LOOSE FOR VALUES FROM (200) TO (300)",
        "ALTER TABLE T DETACH PARTITION T0",
        "COMMIT",
    ] {
        exec(&s, sql).unwrap();
    }
    drop((s, db));
    for _ in 0..2 {
        let db = opened(&dir, true);
        let s = db.session("S");
        assert_eq!(db.index_entries("S.T.ONE_V\u{0}\u{0}GLOBAL").unwrap(), 2);
        assert_eq!(exec(&s, "INSERT INTO T VALUES (120, 3)"), Err(Code::Constraint));
        exec(&s, "INSERT INTO T VALUES (120, 1)").unwrap();
        exec(&s, "ROLLBACK").unwrap();
        db.checkpoint().unwrap();
    }
}
