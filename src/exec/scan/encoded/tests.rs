//! Unit tests of index scans that compare each row's key with its entry's
//! as the index holds it (RULES.md R-TEST-1).

use crate::db::{Database, Session};
use crate::engine::rows::{COLUMN_STORE, ROW_STORE};
use crate::index::BTREE_METHOD;

/// The rows of `S.T`, each key K from 0 to `ROWS` - 1 with V ten times it.
const ROWS: i32 = 1000;

/// A database whose tables `engine` keeps, of `S.T` (K, V) filled and an
/// index on K.
fn filled(engine: &str) -> Database {
    let db = Database::new();
    db.use_engine(engine).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL, V INTEGER)").unwrap();
    let s = db.session("S");
    let values: Vec<String> = (0_i32..ROWS).map(|k| format!("({k}, {k}0)")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    db.create_index("T_K", "S.T", &["K"], BTREE_METHOD).unwrap();
    db
}

/// The rows `sql` gives in `s`, each its values joined by bars.
fn rows(s: &Session, sql: &str) -> Vec<String> {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join("|"));
    }
    out
}

/// Verifies: [DESIGN] RULES.md D821, D34: a range scan whose entries are
/// compared with their rows as bytes finds each row of its keys once, in
/// order and reversed, and none by an entry of a version its reader does
/// not see: a key an update moved, committed after the reader's snapshot,
/// finds its row under its old key alone, and once the reader sees the
/// update, under its new key alone.
#[test]
fn an_encoded_scan_keeps_a_row_under_the_key_it_sees() {
    let (old, moved) = ("SELECT K, V FROM T WHERE K BETWEEN 4 AND 6", "SELECT K, V FROM T WHERE K BETWEEN 1004 AND 1006");
    for engine in [ROW_STORE, COLUMN_STORE] {
        let db = filled(engine);
        let reader = db.session("S");
        reader.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
        assert_eq!(rows(&reader, old), ["4|40", "5|50", "6|60"], "{engine}");
        let writer = db.session("S");
        writer.run("UPDATE T SET K = K + 1000 WHERE K = 5").unwrap();
        writer.commit().unwrap();
        assert_eq!(rows(&reader, old), ["4|40", "5|50", "6|60"], "{engine}: its snapshot's");
        assert!(rows(&reader, moved).is_empty(), "{engine}: no row by the moved key's entry");
        assert_eq!(rows(&reader, "SELECT K FROM T WHERE K BETWEEN 4 AND 6 ORDER BY K DESC"), ["6", "5", "4"], "{engine}");
        reader.commit().unwrap();
        assert_eq!(rows(&reader, moved), ["1005|50"], "{engine}: the update seen");
        assert_eq!(rows(&reader, old), ["4|40", "6|60"], "{engine}");
    }
}
