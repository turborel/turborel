//! ATTACH PARTITION and DETACH PARTITION.

use turborel::error::Code;

use crate::common::{exec, failed, sorted};

use super::sales;

/// Verifies: [EXTENSION] RULES.md D153, D158: a table attached keeps its
/// rows, read through the partitioned table, and adopts an index of a
/// template's definition; one of a row its bound does not hold is refused
/// with SQLSTATE 23T01; one of other columns is refused; a partition
/// detached keeps its rows, no longer the tree's, and its indexes, renamed.
#[test]
fn tables_are_attached_and_detached() {
    let (db, s) = sales();
    exec(&s, "CREATE INDEX BY_AMOUNT ON SALES (AMOUNT)").unwrap();
    exec(&s, "CREATE TABLE OLD (ID INTEGER NOT NULL, SOLD DATE NOT NULL, AMOUNT DECIMAL(9,2))").unwrap();
    exec(&s, "INSERT INTO OLD VALUES (7, DATE '2024-05-01', 70)").unwrap();
    exec(&s, "COMMIT").unwrap();
    exec(&s, "CREATE INDEX OLD_AMOUNT ON OLD (AMOUNT)").unwrap();
    let e = s
        .execute(
            "ALTER TABLE SALES ATTACH PARTITION OLD FOR VALUES FROM (DATE '2024-06-01') TO (DATE '2025-01-01')",
            &mut turborel::params::Params::new(),
        )
        .unwrap_err();
    assert_eq!((e.code, e.sqlstate().as_str()), (Code::Constraint, "23T01"));
    exec(&s, "ROLLBACK").unwrap();
    s.execute(
        "ALTER TABLE SALES ATTACH PARTITION OLD FOR VALUES FROM (DATE '2024-01-01') TO (DATE '2025-01-01')",
        &mut turborel::params::Params::new(),
    )
    .unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM SALES"), ["7"]);
    assert_eq!(db.index_entries("BY_AMOUNT\u{0}S\u{0}OLD").unwrap(), 1);
    assert!(db.index_entries("OLD_AMOUNT").is_err());
    exec(&s, "CREATE TABLE ODD (ID INTEGER, SOLD DATE)").unwrap();
    assert_eq!(
        exec(&s, "ALTER TABLE SALES ATTACH PARTITION ODD FOR VALUES FROM (DATE '2020-01-01') TO (DATE '2021-01-01')"),
        Err(Code::SyntaxRule)
    );
    exec(&s, "ROLLBACK").unwrap();
    exec(&s, "ALTER TABLE SALES DETACH PARTITION OLD").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM SALES"), Vec::<String>::new());
    assert_eq!(sorted(&s, "SELECT ID FROM OLD"), ["7"]);
    assert_eq!(db.index_entries("BY_AMOUNT_OLD").unwrap(), 1);
    let _ = failed;
}
