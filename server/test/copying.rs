//! Rows in and out over a real connection, and the statements and cursors
//! a client describes (RULES.md D188, D194, D195): bulk copy in, export as
//! CSV and as native rows, Describe, several cursors of one statement, and
//! the log position of a commit a replica's session waits for.

use turborel::db::Database;
use turborel::value::{Decimal, Value};
use turborel_client::export::{ExportFormat, read_native};

use crate::common::{Scratch, rows, start, state};

/// A database whose schema X holds T, of an INTEGER K NOT NULL and a
/// VARCHAR(3) S.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, S VARCHAR(3))").unwrap();
    db
}

/// The INTEGER `n`.
fn int(n: i32) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The row of `k` and `s`.
fn row(k: i32, s: &str) -> Vec<Value> {
    vec![int(k), Value::Char(s.to_owned())]
}

/// Verifies: [DESIGN] [ISO/IEC 9075:1992 §13.8 GR4b iii] RULES.md D195:
/// rows copied in go into the table's columns, named or all, each value
/// assigned as INSERT assigns it; a copy is all or nothing, a row refused
/// undoing every row of it, its detail naming the row; a copy abandoned
/// undoes its rows, and so does one the client terminates the session
/// during; the connection takes requests after each.
#[test]
fn rows_are_copied_in_all_or_nothing() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let many: Vec<Vec<Value>> = (0..200).map(|k| row(k, "a")).collect();
    assert_eq!(c.copy_rows("T", &[], many).unwrap(), 200);
    assert_eq!(c.copy_rows("x.t", &["s", "k"], vec![vec![Value::Char("b".into()), int(500)]]).unwrap(), 1);
    let refused = c.copy_rows("T", &[], (0..100).map(|k| row(1000 + k, if k == 70 { "long" } else { "c" }))).unwrap_err();
    assert_eq!(refused.sqlstate, "22001");
    assert_eq!(refused.diagnostics.as_deref().and_then(|d| d.detail.as_deref()), Some("row 71 of the copy"));
    let mut copying = c.copy_in("T", &[]).unwrap();
    copying.send(row(2000, "d")).unwrap();
    copying.abandon().unwrap();
    let missing = c.copy_rows("NOPE", &[], vec![row(1, "a")]).unwrap_err();
    assert_eq!(state(&missing).0, "37000", "{missing}");
    assert_eq!(rows(&c.query("SELECT COUNT(*), MAX(K) FROM T").unwrap()), ["201,500"]);
    let mut ending = server.connect("X");
    ending.query("INSERT INTO T VALUES (3000, 'e')").unwrap();
    let mut copying = ending.copy_in("T", &[]).unwrap();
    // Enough rows that some are sent, and inserted, before the end.
    for k in 0..20_000 {
        copying.send(row(10_000 + k, "e")).unwrap();
    }
    // Not abandoned: the client terminates with the copy under way.
    let _ = std::mem::ManuallyDrop::new(copying);
    ending.terminate().unwrap();
    let kept = c.query("SELECT COUNT(*), MIN(K) FROM T WHERE K >= 3000").unwrap();
    assert_eq!(rows(&kept), ["1,3000"], "the transaction committed at its end, the copy undone");
}

/// Verifies: [DESIGN] [RFC 4180 §2] RULES.md D195: a query's rows export as
/// CSV, a header then a record each, quoted where they must be, a null
/// empty; and as native rows, read back as they were.
#[test]
fn a_query_exports_as_csv_and_native_rows() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    c.query("INSERT INTO T VALUES (1, 'a,b'), (2, NULL)").unwrap();
    let mut csv = Vec::new();
    assert_eq!(c.export("SELECT K, S FROM T ORDER BY K", &mut csv, ExportFormat::Csv).unwrap(), 2);
    assert_eq!(String::from_utf8(csv).unwrap(), "K,S\r\n1,\"a,b\"\r\n2,\r\n");
    let mut native = Vec::new();
    c.export("SELECT K, S FROM T ORDER BY K", &mut native, ExportFormat::Native).unwrap();
    let (columns, back) = read_native(&mut native.as_slice()).unwrap();
    assert_eq!(columns.len(), 2);
    assert_eq!(back.iter().map(|r| format!("{},{}", r[0], r[1])).collect::<Vec<_>>(), ["1,a,b", "2,NULL"]);
    assert!(c.export("DELETE FROM T", &mut Vec::new(), ExportFormat::Csv).is_err(), "not a query");
}

/// Verifies: [DESIGN] [ISO/IEC 9075:1992 §17.7 GR2, §13.2 GR1] RULES.md
/// D188: a statement is described again, and an open cursor; a statement
/// has several cursors open at once, each read on its own; it is not
/// destroyed while one is open.
#[test]
fn a_statement_has_several_cursors_described() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    c.query("INSERT INTO T VALUES (1, 'a'), (2, 'b'), (3, 'c')").unwrap();
    let query = c.prepare("SELECT K FROM T WHERE K >= ? ORDER BY K").unwrap();
    let again = c.describe(&query).unwrap();
    assert_eq!((again.parameters().len(), again.columns().len()), (1, 1));
    let first = c.open_cursor_numbered((&query, 1), &[int(1)], 1).unwrap();
    let second = c.open_cursor_numbered((&query, 2), &[int(2)], 1).unwrap();
    assert_eq!((first.rows[0][0].to_string(), second.rows[0][0].to_string()), ("1".into(), "2".into()));
    assert_eq!(c.describe_cursor((&query, 2)).unwrap().len(), 1);
    assert_eq!(state(&c.deallocate(query.clone()).unwrap_err()).0, "24000", "a cursor of it open");
    let rest = c.fetch_numbered((&query, 1), 0).unwrap();
    assert_eq!(rest.rows.len(), 2);
    assert_eq!(state(&c.describe_cursor((&query, 1)).unwrap_err()).0, "24000", "closed as its rows ended");
    c.close_numbered((&query, 2)).unwrap();
    c.deallocate(query).unwrap();
}

/// Verifies: [DESIGN] RULES.md D194: a commit's completion carries its log
/// position; a session whose REPLICATION.READ_AFTER names a position the
/// database has applied reads at once, and one it has not fails past
/// STATEMENT.TIMEOUT_MS with SQLSTATE TBR01.
#[test]
fn a_session_reads_after_a_commits_position() {
    let scratch = Scratch::new("read_after");
    let db = Database::open(&scratch.path("db")).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, S VARCHAR(3))").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    c.set_autocommit(true);
    c.query("INSERT INTO T VALUES (1, 'a')").unwrap();
    let position = c.commit_position().unwrap();
    let mut reader = server.connect("X");
    reader.query(&format!("SET REPLICATION.READ_AFTER = {position}")).unwrap();
    assert_eq!(rows(&reader.query("SELECT COUNT(*) FROM T").unwrap()), ["1"]);
    reader.query("ROLLBACK").unwrap();
    reader.query("SET STATEMENT.TIMEOUT_MS = 50").unwrap();
    reader.query(&format!("SET REPLICATION.READ_AFTER = {}", position + 1_000_000)).unwrap();
    let late = reader.query("SELECT COUNT(*) FROM T").unwrap_err();
    assert_eq!(state(&late), ("TBR01", -920));
    reader.query("SET REPLICATION.READ_AFTER = 0").unwrap();
    assert_eq!(rows(&reader.query("SELECT COUNT(*) FROM T").unwrap()), ["1"], "a SET never waits, so it is cleared");
}
