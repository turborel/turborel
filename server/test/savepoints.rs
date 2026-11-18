//! The savepoint statements over a connection [ISO/IEC 9075-2:2023
//! §17.5, §17.6, §17.8]: run as queries and prepared, their failures
//! sent with their SQLSTATE, the transaction going on across them.

use turborel::db::Database;
use turborel::value::{Decimal, Value};

use crate::common::{done, rows, start, state};

/// Verifies: [ISO/IEC 9075-2:2023 §17.5, §17.6 GR2, §17.8 GR3b, GR3d];
/// [DESIGN] RULES.md D45, D88. Over a connection, SAVEPOINT and ROLLBACK
/// TO SAVEPOINT sent as queries complete, the rows written since undone
/// and the transaction going on; prepared, they execute as written; a
/// RELEASE of no savepoint is sent as 3B001 (SQLCODE -917), the
/// connection going on; the COMMIT then commits what was kept.
#[test]
fn savepoints_over_a_connection() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL)").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    assert_eq!(done(&c.query("INSERT INTO T VALUES (1)").unwrap()), 0);
    assert_eq!(done(&c.query("SAVEPOINT S").unwrap()), 0);
    c.query("INSERT INTO T VALUES (2)").unwrap();
    assert_eq!(done(&c.query("ROLLBACK TO SAVEPOINT S").unwrap()), 0);
    let establish = c.prepare("SAVEPOINT P").unwrap();
    let back = c.prepare("ROLLBACK TO SAVEPOINT P").unwrap();
    let insert = c.prepare("INSERT INTO T VALUES (?)").unwrap();
    c.execute(&establish, &[]).unwrap();
    c.execute(&insert, &[Value::Exact(Decimal::new(3, 0))]).unwrap();
    c.execute(&back, &[]).unwrap();
    c.execute(&insert, &[Value::Exact(Decimal::new(4, 0))]).unwrap();
    let refused = c.query("RELEASE SAVEPOINT NONE").unwrap_err();
    assert_eq!(state(&refused), ("3B001", -917), "{}", refused.message);
    c.query("RELEASE SAVEPOINT S").unwrap();
    c.query("COMMIT").unwrap();
    assert_eq!(rows(&c.query("SELECT K FROM T ORDER BY K").unwrap()), ["1", "4"]);
}
