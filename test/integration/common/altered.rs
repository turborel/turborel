//! Helpers of the tests of ALTER TABLE's forms (`alter_columns.rs`,
//! `renamed_tables.rs`).

use std::path::Path;

use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::error::SqlError;

use super::{after_crash, number, rows};

/// A database in memory of schema A.
pub fn schema_a() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
pub fn committed_each(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The rows of `sql` in `s`, which must succeed.
pub fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// The error `sql` fails with in `s`, its message holding `cited`, the
/// transaction rolled back.
pub fn alter_failed(s: &Session, sql: &str, cited: &str) -> SqlError {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} succeeded"));
    s.rollback().unwrap();
    assert!(e.to_string().contains(cited), "{sql}: {e} does not cite {cited}");
    e
}

/// The database in `dir`, opened after a crash, on a file system that
/// keeps only what was synced, with no checkpoint but those asked for.
pub fn reopened_after_crash(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    (db, opened)
}
