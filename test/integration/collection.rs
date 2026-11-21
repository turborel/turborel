//! \[DESIGN\] Garbage collection under many sessions (RULES.md D29, D445):
//! one transaction's end collects a table at a time, the others pass it
//! over, and what they pass over is collected by a later end.

use std::thread;

use turborel::db::Database;

use crate::common::rows;

/// The rows of T.
const ROWS: usize = 64;

/// Verifies: [DESIGN] RULES.md D29, D445: sixteen sessions changing the
/// key of an index's rows at once, each end passing over a collection
/// under way, leave no version behind: once they end, one transaction's
/// end collects the rest, and the index holds one entry a row.
#[test]
fn collections_passed_over_are_done_later() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, X INTEGER)").unwrap();
    db.create_index("T_X", "APP.T", &["X"], "btree").unwrap();
    let s = db.session("APP");
    let values: Vec<String> = (0..ROWS).map(|i| format!("({i}, 0)")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    let threads: Vec<_> = (0..16)
        .map(|t| {
            let db = db.clone();
            thread::spawn(move || {
                let s = db.session("APP");
                for n in 0..40 {
                    let sql = format!("UPDATE T SET X = X + 1 WHERE ID = {}", (n * 5 + t * 3) % ROWS);
                    while s.run(&sql).and_then(|_| s.commit()).is_err() {
                        let _ = s.rollback();
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    rows(&s, "SELECT ID FROM T WHERE ID = 0").unwrap();
    s.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS, "every superseded version's entry taken back");
    assert_eq!(rows(&s, "SELECT SUM(X) FROM T"), Ok(vec!["640".to_owned()]));
}
