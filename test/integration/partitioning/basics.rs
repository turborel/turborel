//! Rows written to a partitioned table and read back.

use crate::common::{exec, sorted};

use super::{plan, sales};

/// Verifies: [EXTENSION] RULES.md D153, D155, D156: rows inserted into a
/// partitioned table are each in the partition whose bound holds them, the
/// DEFAULT holding the rest; the table reads them all; a read of one year
/// reads that year's partition alone.
#[test]
fn rows_go_to_their_partitions() {
    let (_db, s) = sales();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10), (2, DATE '2026-05-01', 20), (3, DATE '2030-01-01', 30)").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM SALES"), ["1", "2", "3"]);
    assert_eq!(sorted(&s, "SELECT ID FROM S2025"), ["1"]);
    assert_eq!(sorted(&s, "SELECT ID FROM S2026"), ["2"]);
    assert_eq!(sorted(&s, "SELECT ID FROM SOTHER"), ["3"]);
    let shown = plan(&s, "SELECT ID FROM SALES WHERE SOLD = DATE '2026-05-01'");
    assert!(shown.contains("S2026") && !shown.contains("S2025") && !shown.contains("SOTHER"), "{shown}");
    s.execute("UPDATE SALES SET SOLD = DATE '2025-06-01' WHERE ID = 2", &mut turborel::params::Params::new()).unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM S2025"), ["1", "2"]);
    exec(&s, "DELETE FROM SALES WHERE ID = 1").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM SALES"), ["2", "3"]);
}
