//! DROP TABLE of partitioned tables and partitions, and MERGE into them.

use turborel::error::Code;

use crate::common::{exec, sorted};

use super::sales;

/// Verifies: [EXTENSION] RULES.md D153, D157: DROP TABLE of a partition
/// takes it and its rows from its tree; of a partitioned table, the whole
/// tree; neither while a foreign key references the tree.
#[test]
fn partitions_and_trees_are_dropped() {
    let (db, s) = sales();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10), (2, DATE '2026-03-01', 20)").unwrap();
    exec(&s, "COMMIT").unwrap();
    exec(&s, "DROP TABLE S2025 CASCADE").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM SALES"), ["2"]);
    exec(&s, "INSERT INTO SALES VALUES (3, DATE '2025-03-01', 30)").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM SOTHER"), ["3"]);
    exec(&s, "CREATE TABLE NOTE (ID INTEGER, SOLD DATE, FOREIGN KEY (ID, SOLD) REFERENCES SALES (ID, SOLD))").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert!(exec(&s, "DROP TABLE S2026 CASCADE").is_err());
    exec(&s, "ROLLBACK").unwrap();
    exec(&s, "DROP TABLE NOTE CASCADE").unwrap();
    exec(&s, "DROP TABLE SALES CASCADE").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(exec(&s, "SELECT ID FROM S2026"), Err(Code::Syntax));
    assert_eq!(sorted(&s, "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.PARTITIONS"), Vec::<String>::new());
    let _ = db;
}

/// Verifies: [EXTENSION] RULES.md D155: MERGE into a partitioned table
/// inserts each row in the partition that holds it, updates rows in
/// place, and moves a row whose key it changes; a row no partition holds
/// fails with SQLSTATE 23T01.
#[test]
fn merge_routes_and_moves_rows() {
    let (_db, s) = sales();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10)").unwrap();
    exec(&s, "CREATE TABLE SRC (ID INTEGER, SOLD DATE, AMOUNT DECIMAL(9,2))").unwrap();
    exec(&s, "INSERT INTO SRC VALUES (1, DATE '2026-05-01', 11), (2, DATE '2026-06-01', 22)").unwrap();
    exec(
        &s,
        "MERGE INTO SALES T USING SRC ON T.ID = SRC.ID \
         WHEN MATCHED THEN UPDATE SET SOLD = SRC.SOLD, AMOUNT = SRC.AMOUNT \
         WHEN NOT MATCHED THEN INSERT VALUES (SRC.ID, SRC.SOLD, SRC.AMOUNT)",
    )
    .unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM S2026"), ["1", "2"]);
    assert_eq!(sorted(&s, "SELECT ID FROM S2025"), Vec::<String>::new());
    exec(&s, "COMMIT").unwrap();
    exec(&s, "CREATE TABLE NARROW (K INTEGER) PARTITION BY RANGE (K)").unwrap();
    exec(&s, "CREATE TABLE N0 PARTITION OF NARROW FOR VALUES FROM (0) TO (10)").unwrap();
    exec(&s, "COMMIT").unwrap();
    let e = s
        .execute(
            "MERGE INTO NARROW USING SRC ON 1 = 0 WHEN NOT MATCHED THEN INSERT VALUES (SRC.ID * 100)",
            &mut turborel::params::Params::new(),
        )
        .unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "23T01");
}
