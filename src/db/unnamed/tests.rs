//! Unit tests of the unnamed cursors a session keeps track of (RULES.md
//! R-TEST-1).

use crate::db::Database;

/// Verifies: [DESIGN] X3.135-1986 §8.8 GR3b: a session keeps track of
/// the unnamed cursors open in its transaction alone. A transaction that
/// runs ten thousand queries, as one reading at a snapshot for each does
/// under autocommit, each cursor closed before the next opens, holds one
/// entry at a time, so a statement that looks at its open cursors looks
/// at no more; one still open is kept, and seen open.
#[test]
fn closed_cursors_are_forgotten() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (1), (2)").unwrap();
    for _ in 0_u32..10_000_u32 {
        let cursor = s.open_cursor("SELECT K FROM T").unwrap();
        drop(cursor);
    }
    assert_eq!(s.inner.borrow().anonymous.len(), 1, "the last one's alone");
    let kept = s.open_cursor("SELECT K FROM T").unwrap();
    let other = s.open_cursor("SELECT K FROM T").unwrap();
    drop(other);
    let _third = s.open_cursor("SELECT K FROM T").unwrap();
    assert_eq!(s.inner.borrow().anonymous.len(), 2, "the open ones");
    assert!(s.cursors_open());
    drop(kept);
}
