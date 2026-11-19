//! Foreign keys, CHECK constraints and triggers of a partitioned table.

use turborel::error::Code;

use crate::common::{exec, sorted};

use super::sales;

/// Verifies: [EXTENSION] RULES.md D153, D157: a foreign key to a
/// partitioned table finds the referenced row in its partition, and one
/// from it is checked of each partition's rows; a CHECK of it holds of
/// each partition's rows, through it or written to the partition itself.
#[test]
fn constraints_hold_through_the_tree() {
    let (_db, s) = sales();
    exec(&s, "CREATE TABLE NOTE (ID INTEGER, SOLD DATE, FOREIGN KEY (ID, SOLD) REFERENCES SALES (ID, SOLD))").unwrap();
    exec(&s, "ALTER TABLE SALES ADD CONSTRAINT POSITIVE CHECK (AMOUNT > 0)").unwrap();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10)").unwrap();
    exec(&s, "INSERT INTO NOTE VALUES (1, DATE '2025-03-01')").unwrap();
    assert_eq!(exec(&s, "INSERT INTO NOTE VALUES (2, DATE '2025-03-01')"), Err(Code::Constraint));
    assert_eq!(exec(&s, "INSERT INTO SALES VALUES (2, DATE '2025-03-01', -1)"), Err(Code::Constraint));
    assert_eq!(exec(&s, "INSERT INTO S2026 VALUES (2, DATE '2026-03-01', -1)"), Err(Code::Constraint));
    assert_eq!(exec(&s, "DELETE FROM SALES WHERE ID = 1"), Err(Code::Constraint));
    let e = s.execute("INSERT INTO S2026 VALUES (3, DATE '2025-03-01', 1)", &mut turborel::params::Params::new()).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "23T01");
    exec(&s, "CREATE TABLE BUYER (ID INTEGER PRIMARY KEY)").unwrap();
    exec(&s, "CREATE TABLE ORDERS (ID INTEGER, BUYER INTEGER REFERENCES BUYER) PARTITION BY HASH (ID)").unwrap();
    exec(&s, "CREATE TABLE O0 PARTITION OF ORDERS FOR VALUES WITH (MODULUS 1, REMAINDER 0)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO ORDERS VALUES (1, 9)"), Err(Code::Constraint));
    exec(&s, "INSERT INTO BUYER VALUES (9)").unwrap();
    exec(&s, "INSERT INTO ORDERS VALUES (1, 9)").unwrap();
    assert_eq!(sorted(&s, "SELECT BUYER FROM O0"), ["9"]);
}

/// Verifies: [EXTENSION] RULES.md D155: a row trigger of a partitioned
/// table fires for the rows of its partitions, written through it or to a
/// partition; a row an UPDATE moves to another partition is an update of
/// the partitioned table.
#[test]
fn triggers_fire_for_partitions() {
    let (_db, s) = sales();
    exec(&s, "CREATE TABLE SEEN (WHAT CHAR(1), ID INTEGER)").unwrap();
    exec(&s, "CREATE TRIGGER I AFTER INSERT ON SALES REFERENCING NEW ROW AS N FOR EACH ROW INSERT INTO SEEN VALUES ('I', N.ID)").unwrap();
    exec(&s, "CREATE TRIGGER U AFTER UPDATE ON SALES REFERENCING NEW ROW AS N FOR EACH ROW INSERT INTO SEEN VALUES ('U', N.ID)").unwrap();
    exec(&s, "CREATE TRIGGER D AFTER DELETE ON SALES REFERENCING OLD ROW AS O FOR EACH ROW INSERT INTO SEEN VALUES ('D', O.ID)").unwrap();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10)").unwrap();
    exec(&s, "INSERT INTO S2026 VALUES (2, DATE '2026-03-01', 10)").unwrap();
    exec(&s, "UPDATE SALES SET SOLD = DATE '2026-04-01' WHERE ID = 1").unwrap();
    exec(&s, "DELETE FROM SALES WHERE ID = 2").unwrap();
    assert_eq!(sorted(&s, "SELECT WHAT || CAST(ID AS CHAR(1)) FROM SEEN"), ["D2", "I1", "I2", "U1"]);
}
