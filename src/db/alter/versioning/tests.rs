//! Unit tests of the history of a system-versioned table (RULES.md D82):
//! the historical system rows its updates and deletes keep, as no
//! statement reads them yet.

use std::sync::Arc;

use crate::catalog::QualifiedName;
use crate::db::Database;
use crate::db::write::system_time::history_name;
use crate::txn::NO_TRANSACTION;
use crate::value::Value;

/// The table A.T, system-versioned.
const VERSIONED: &str = "CREATE TABLE T (ID INTEGER, V INTEGER, S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, \
    E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (S, E)) WITH SYSTEM VERSIONING";

/// The rows of the history of A.T in `db`, as text, ordered by start: V,
/// S, E.
fn history(db: &Database) -> Vec<[String; 3]> {
    let st = db.snapshot().unwrap();
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let mut rows: Vec<[String; 3]> = Arc::clone(st.storage_of(&history_name(&table)).unwrap())
        .scan(NO_TRANSACTION, &[0, 1, 2, 3])
        .map(|r| {
            let row = r.unwrap().1;
            [1, 2, 3].map(|c| row.get(c).map_or_else(String::new, Value::to_string))
        })
        .collect();
    rows.sort_by(|a, b| a[1].cmp(&b[1]));
    rows
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.14 GR10a iii 1 C I, §15.8 GR3a,
/// §15.14 GR10a iii 1 B] [DESIGN] RULES.md D82
/// Each update by a later transaction keeps the row it replaces in the
/// history, ending where the new one begins, and a delete the row it
/// deletes; a row changed again by the transaction that began it is
/// changed in place, keeping none.
#[test]
fn updates_and_deletes_keep_historical_rows() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    for sql in [VERSIONED, "INSERT INTO T (ID, V) VALUES (1, 10)", "UPDATE T SET V = 11"] {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
    assert_eq!(history(&db), Vec::<[String; 3]>::new(), "changed in place");
    s.run("UPDATE T SET V = 20").unwrap();
    s.commit().unwrap();
    s.run("DELETE FROM T").unwrap();
    s.commit().unwrap();
    let kept = history(&db);
    assert_eq!(kept.iter().map(|r| r[0].as_str()).collect::<Vec<_>>(), ["11", "20"]);
    assert_eq!(kept[0][2], kept[1][1], "the first ends as the second begins");
    assert!(kept.iter().all(|r| r[1] < r[2] && !r[2].starts_with("9999")), "{kept:?}");
    s.run("ALTER TABLE T DROP SYSTEM VERSIONING CASCADE").unwrap();
    s.run("ALTER TABLE T ADD SYSTEM VERSIONING").unwrap();
    s.commit().unwrap();
    assert_eq!(history(&db), Vec::<[String; 3]>::new(), "made anew");
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.17.2.3] [DESIGN] RULES.md D82
/// A transaction rolled back keeps no historical row.
#[test]
fn a_rollback_keeps_no_historical_row() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    for sql in [VERSIONED, "INSERT INTO T (ID, V) VALUES (1, 10)"] {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
    s.run("UPDATE T SET V = 20").unwrap();
    s.rollback().unwrap();
    assert_eq!(history(&db), Vec::<[String; 3]>::new());
}
