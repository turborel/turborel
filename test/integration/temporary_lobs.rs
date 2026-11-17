//! Large objects in temporary tables [ISO/IEC 9075-2:2023 §4.18.3, §4.3,
//! §4.4] (RULES.md D79): a session's rows of a global temporary table
//! holding a CLOB of many chunks and a BLOB, read back, updated and
//! deleted, seen by no other session; and the table's statistics gathered.

use turborel::db::Database;
use turborel::value::lob::Lob;
use turborel::value::{Decimal, Value};

use crate::common::rows;

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.18.3, §4.3, §4.4] a
/// session's rows of a global temporary table keep large objects, long and
/// short, read back by length and by substring; updated and deleted; while
/// another session's instance of the table is empty; the table's
/// statistics are gathered from its rows, or from none.
#[test]
fn large_objects_in_a_temporary_table_4_18() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE GLOBAL TEMPORARY TABLE G (K INTEGER, C CLOB, B BLOB) ON COMMIT PRESERVE ROWS").unwrap();
    db.analyze("A.G").unwrap();
    let a = db.session("A");
    let long = "é".repeat(200_000);
    let insert = a.prepare("INSERT INTO G VALUES (?, ?, ?)").unwrap();
    for (k, text) in [(1, "short"), (2, long.as_str())] {
        let values = [Value::Exact(Decimal::new(k, 0)), Value::Lob(Lob::of_text(text)), Value::Lob(Lob::of_octets(&[1, 2, 3]))];
        a.execute_prepared(&insert, &values).unwrap();
    }
    a.commit().unwrap();
    let lengths = "SELECT K, CHAR_LENGTH(C), OCTET_LENGTH(C), OCTET_LENGTH(B) FROM G ORDER BY K";
    assert_eq!(rows(&a, lengths).unwrap(), ["1,5,5,3", "2,200000,400000,3"]);
    assert_eq!(rows(&a, "SELECT CAST(SUBSTRING(C FROM 199999) AS VARCHAR(4)) FROM G WHERE K = 2").unwrap(), ["éé"]);
    a.run("UPDATE G SET C = C || 'x', B = X'09' WHERE K = 1").unwrap();
    assert_eq!(rows(&a, "SELECT C, B FROM G WHERE K = 1").unwrap(), ["shortx,09"]);
    db.analyze("A.G").unwrap();
    assert_eq!(rows(&db.session("A"), "SELECT COUNT(*) FROM G").unwrap(), ["0"], "another session's instance");
    a.run("DELETE FROM G WHERE K = 2").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT K FROM G").unwrap(), ["1"]);
}
