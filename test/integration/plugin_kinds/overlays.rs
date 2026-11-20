//! \[EXTENSION\] A transaction's writes to a table of an extension's
//! storage engine (RULES.md D224), FILES of the example, held apart until
//! commit: rows written again after a savepoint, put back as it is rolled
//! back to, and a transaction that reads and writes nothing applying none.

use super::{calls, kinds};
use crate::common::rows;

/// Verifies: [EXTENSION] RULES.md D224 [ISO/IEC 9075-2:2023 §17.8]: a
/// row a transaction wrote, written again or deleted after a savepoint,
/// is put back as it was at the savepoint when the transaction rolls back
/// to it, and committed so; a transaction that only reads the table makes
/// no write call at its commit.
#[test]
fn rows_written_again_are_put_back_at_a_savepoint() {
    let (_scratch, db) = kinds("plugin-overlays", "");
    let s = db.session("U");
    s.run("CREATE TABLE T (K INTEGER, V VARCHAR(20)) USING FILES").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (1, 'first'), (2, 'second')").unwrap();
    s.run("SAVEPOINT A").unwrap();
    s.run("UPDATE T SET V = 'changed' WHERE K = 1").unwrap();
    s.run("DELETE FROM T WHERE K = 2").unwrap();
    s.run("INSERT INTO T VALUES (3, 'third')").unwrap();
    assert_eq!(rows(&s, "SELECT K, V FROM T ORDER BY K").unwrap(), ["1,changed", "3,third"]);
    s.run("ROLLBACK TO SAVEPOINT A").unwrap();
    assert_eq!(rows(&s, "SELECT K, V FROM T ORDER BY K").unwrap(), ["1,first", "2,second"]);
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT K, V FROM T ORDER BY K").unwrap(), ["1,first", "2,second"]);
    let before = calls(&s);
    s.commit().unwrap();
    assert_eq!(calls(&s), before, "nothing to apply");
}
