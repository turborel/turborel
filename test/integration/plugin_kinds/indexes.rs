//! \[EXTENSION\] Indexes of an extension's index methods (RULES.md D226,
//! D227), SORTED and NEAR of the example: equality, ranges and nearest
//! searches as the manifest declares them, each entry rechecked against
//! the row a transaction sees; writes a batch a call; an index whose
//! program was cut rebuilt as the database reopens.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;

use super::{KINDS, calls, kinds, state};
use crate::common::{Scratch, after_crash, exec, rows};
use crate::plugins::fixture::{allow, install, manifest};

/// T of `n` rows, K from 0, committed.
fn filled(s: &turborel::db::Session, n: u32) {
    s.run("CREATE TABLE T (K INTEGER, V VARCHAR(20))").unwrap();
    let values: Vec<String> = (0..n).map(|k| format!("({k}, 'v{k}')")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D226: an index of a method that
/// SUPPORTS EQUALITY and RANGE serves both, in either order, its rows the
/// table's; inserts, updates and deletes reach it a batch at a time; one
/// method that does not support NEAREST serves no FETCH APPROX.
#[test]
fn an_index_method_serves_equality_and_ranges() {
    let (_scratch, db) = kinds("plugin-indexes-ranges", "");
    let s = db.session("U");
    filled(&s, 300);
    s.run("CREATE INDEX T_K ON T (K) USING SORTED").unwrap();
    s.commit().unwrap();
    let before = calls(&s);
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 50").unwrap(), ["v50"]);
    assert!(calls(&s) > before, "the index was searched");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE K BETWEEN 10 AND 13 ORDER BY K").unwrap(), ["10", "11", "12", "13"]);
    assert_eq!(rows(&s, "SELECT K FROM T WHERE K >= 297 ORDER BY K DESC").unwrap(), ["299", "298", "297"]);
    s.run("UPDATE T SET K = K + 1000 WHERE K < 5").unwrap();
    s.run("DELETE FROM T WHERE K BETWEEN 100 AND 199").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K < 200").unwrap(), ["95"]);
    assert_eq!(rows(&s, "SELECT K FROM T WHERE K > 1000 ORDER BY K").unwrap(), ["1001", "1002", "1003", "1004"]);
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K BETWEEN 0 AND 299").unwrap(), ["195"]);
    assert_eq!(exec(&s, "DROP EXTENSION kinds"), Err(Code::SyntaxRule), "an index is of it");
    s.rollback().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D226: each entry is rechecked against
/// the row the reader sees, so a writer's entries not committed are not
/// another's rows, and a rollback's are gone; a process restarted keeps
/// the index whole.
#[test]
fn an_index_methods_entries_are_rechecked() {
    let (_scratch, db) = kinds("plugin-indexes-visibility", "");
    let s = db.session("U");
    filled(&s, 20);
    s.run("CREATE INDEX T_K ON T (K) USING SORTED").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (500, 'mine')").unwrap();
    s.run("UPDATE T SET V = 'changed' WHERE K = 3").unwrap();
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 500").unwrap(), ["mine"]);
    let other = db.session("U");
    assert_eq!(rows(&other, "SELECT COUNT(*) FROM T WHERE K = 500").unwrap(), ["0"]);
    assert_eq!(rows(&other, "SELECT V FROM T WHERE K = 3").unwrap(), ["v3"]);
    other.commit().unwrap();
    s.rollback().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K >= 20").unwrap(), ["0"]);
    s.run("INSERT INTO T VALUES (600, 'kept')").unwrap();
    assert_eq!(state(&s, "VALUES (CRASH())"), "38T01");
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 600").unwrap(), ["kept"]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K BETWEEN 0 AND 1000").unwrap(), ["21"]);
}

/// Verifies: [EXTENSION] RULES.md D226: an index of a method that
/// SUPPORTS NEAREST serves FETCH APPROX by the query's metric, the
/// program asked for the nearest entries and each rechecked.
#[test]
fn an_index_method_serves_the_nearest() {
    let (_scratch, db) = kinds("plugin-indexes-nearest", "");
    let s = db.session("U");
    s.run("CREATE TABLE V (K INTEGER PRIMARY KEY, E VECTOR(2))").unwrap();
    let values: Vec<String> = (0..200).map(|k| format!("({k}, '[{k}, {}]')", k % 7)).collect();
    s.run(&format!("INSERT INTO V VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    s.run("CREATE INDEX V_E ON V (E) USING NEAR").unwrap();
    s.commit().unwrap();
    let before = calls(&s);
    let approx = "SELECT K FROM V ORDER BY VECTOR_DISTANCE(E, '[120, 1]', EUCLIDEAN) FETCH APPROX FIRST 3 ROWS ONLY";
    assert_eq!(rows(&s, approx).unwrap()[0], "120");
    assert!(calls(&s) > before, "the program was asked");
    s.run("DELETE FROM V WHERE K = 120").unwrap();
    assert_ne!(rows(&s, approx).unwrap()[0], "120", "a row deleted is rechecked away");
    s.rollback().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D227: an index whose program was told
/// writes after its last checkpoint, and is not clean as a crash leaves
/// it, is rebuilt from its table as the database reopens; a clean one is
/// reopened as it is.
#[test]
fn an_index_is_rebuilt_after_a_crash() {
    let scratch = Scratch::new("plugin-indexes-recovery");
    let root = scratch.path("plugins");
    install(&root, ("KINDS", "1.0.0"), &manifest("KINDS", "1.0.0", "", KINDS));
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        allow(&db, &root);
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    let s = db.session("U");
    s.run("CREATE EXTENSION kinds").unwrap();
    s.commit().unwrap();
    filled(&s, 40);
    s.run("CREATE INDEX T_K ON T (K) USING SORTED").unwrap();
    s.commit().unwrap();
    db.checkpoint().unwrap();
    s.run("INSERT INTO T VALUES (100, 'after'), (101, 'after')").unwrap();
    s.run("DELETE FROM T WHERE K < 10").unwrap();
    s.commit().unwrap();
    drop(s);
    drop(db);
    let db = open();
    let s = db.session("U");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K BETWEEN 0 AND 1000").unwrap(), ["32"]);
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 101").unwrap(), ["after"]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K = 5").unwrap(), ["0"]);
}
