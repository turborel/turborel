//! Updatable cursors over a partitioned table.

use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::{exec, sorted};

use super::sales;

/// The target of a fetch of an ID.
fn targets() -> Params {
    Params::new().declare("ID_OUT", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null)
}

/// Verifies: [EXTENSION] RULES.md D156: a cursor over a partitioned table
/// is updatable; a positioned UPDATE changes the row under it in its
/// partition, one that changes its partition key moves it to the
/// partition its new key routes to; a positioned DELETE takes it from its
/// partition.
#[test]
fn positioned_writes_change_the_row_in_its_partition() {
    let (_db, s) = sales();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10), (2, DATE '2026-03-01', 20), (3, DATE '2030-03-01', 30)").unwrap();
    exec(&s, "COMMIT").unwrap();
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM SALES FOR UPDATE").unwrap();
    let mut p = targets();
    s.execute("OPEN C", &mut p).unwrap();
    s.execute("FETCH C INTO ID_OUT", &mut p).unwrap();
    assert_eq!(p.value("ID_OUT").to_string(), "1");
    s.execute("UPDATE SALES SET AMOUNT = 11 WHERE CURRENT OF C", &mut p).unwrap();
    s.execute("FETCH C INTO ID_OUT", &mut p).unwrap();
    s.execute("UPDATE SALES SET SOLD = DATE '2025-06-01' WHERE CURRENT OF C", &mut p).unwrap();
    s.execute("FETCH C INTO ID_OUT", &mut p).unwrap();
    assert_eq!(p.value("ID_OUT").to_string(), "3");
    s.execute("DELETE FROM SALES WHERE CURRENT OF C", &mut p).unwrap();
    s.execute("CLOSE C", &mut p).unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT ID, AMOUNT FROM S2025"), ["1,11.00", "2,20.00"]);
    assert_eq!(sorted(&s, "SELECT ID FROM S2026"), Vec::<String>::new());
    assert_eq!(sorted(&s, "SELECT ID FROM SOTHER"), Vec::<String>::new());
}
