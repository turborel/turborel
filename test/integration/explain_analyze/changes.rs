//! \[EXTENSION\] EXPLAIN ANALYZE of a data change statement (RULES.md
//! D622): INSERT, UPDATE, DELETE and MERGE run for real in the session's
//! transaction, which may roll them back, each showing the rows it
//! changed above the plan of the rows it read, each operator counted.

use turborel::params::Params;

use super::{analyzed, database, direct_lines, op};
use crate::common::{exec, rows};

/// Verifies: [EXTENSION] RULES.md D622: EXPLAIN ANALYZE UPDATE, as a
/// direct statement, changes the rows in the session's transaction, its
/// first line the rows it changed, the plan of its object rows below it
/// counted, its last line the rows changed; and a rollback undoes it.
#[test]
fn an_update_runs_in_the_transaction_and_shows_its_plan() {
    let db = database();
    let s = db.session("ANN");
    let lines = direct_lines(&s, "EXPLAIN ANALYZE UPDATE S SET X = 9 WHERE X = 1").unwrap();
    assert_eq!(lines.first().map(String::as_str), Some("-- physical:"));
    assert_eq!(lines.get(1).map(String::as_str), Some("Update ANN.S  (actual rows changed=33)"), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("  ") && l.contains("(actual rows=33")), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("-- ran: 33 rows changed"));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM S WHERE X = 9").unwrap(), ["33"]);
    s.rollback().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM S WHERE X = 9").unwrap(), ["0"]);
}

/// Verifies: [EXTENSION] RULES.md D622: through the library, a DELETE's
/// first operator is the statement, its rows those it deleted, and its
/// object rows' scan below it; an INSERT of a query's rows counts the
/// query; a MERGE its joined rows; each kept until rolled back.
#[test]
fn each_data_change_counts_its_rows_and_its_plan() {
    let db = database();
    let s = db.session("ANN");
    let deleted = analyzed(&s, "DELETE FROM S WHERE X = 2");
    let top = deleted.operators.first().unwrap();
    assert_eq!((top.label.as_str(), top.rows, top.depth), ("Delete from ANN.S", 33, 0), "{}", deleted.text);
    assert_eq!(op(&deleted, "SeqScan ANN.S").depth, 1 + op(&deleted, "Filter").depth, "{}", deleted.text);
    let inserted = analyzed(&s, "INSERT INTO S SELECT K, 7 FROM U WHERE W = 3");
    assert_eq!((inserted.rows, op(&inserted, "Insert into ANN.S").rows), (14, 14), "{}", inserted.text);
    assert_eq!(op(&inserted, "SeqScan ANN.U").rows, 100, "{}", inserted.text);
    let merged = analyzed(&s, "MERGE INTO S USING U ON S.K = U.K WHEN MATCHED THEN UPDATE SET X = U.W");
    assert!(merged.rows > 0 && merged.operators.len() > 1, "{}", merged.text);
    assert_eq!(merged.operators.first().unwrap().label, "Merge into ANN.S");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM S").unwrap(), ["81"]);
    s.rollback().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM S").unwrap(), ["100"]);
}

/// Verifies: [EXTENSION] RULES.md D622, D621: an UPDATE that finds its
/// rows by an index shows the index's reads.
#[test]
fn an_update_by_an_index_shows_its_reads() {
    let db = database();
    let s = db.session("ANN");
    let a = analyzed(&s, "UPDATE T SET V = 0 WHERE K = 5");
    let scan = op(&a, "IndexScan ANN.T");
    assert_eq!((a.rows, scan.rows, scan.notes.entries, scan.notes.fetched), (1, 1, 1, 1), "{}", a.text);
    assert!(scan.notes.leaves >= 1 && a.text.contains("entries=1"), "{}", a.text);
}

/// Verifies: [EXTENSION] RULES.md D622, D15: a data change that raises
/// makes EXPLAIN ANALYZE raise the same exception and change nothing, by
/// the library and as a direct statement; and a statement that changes no
/// row shows none changed.
#[test]
fn a_data_change_that_raises_raises_the_same_and_changes_nothing() {
    let db = database();
    let s = db.session("ANN");
    let sql = "UPDATE T SET V = V / (K - K)";
    let run = exec(&s, sql).unwrap_err();
    assert_eq!(s.analyzed(sql, &Params::new()).map_err(|e| e.code).unwrap_err(), run);
    assert_eq!(direct_lines(&s, &format!("EXPLAIN ANALYZE {sql}")).unwrap_err(), run);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE V = K").unwrap(), ["1000"]);
    let none = analyzed(&s, "DELETE FROM T WHERE K < 0");
    assert!(none.rows == 0 && none.text.ends_with("-- ran: 0 rows changed\n"), "{}", none.text);
}
