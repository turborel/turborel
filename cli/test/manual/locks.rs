//! The examples of locks and of data change delta tables, run as printed:
//! row locks, LOCK TABLE and advisory locks, and OLD, NEW and FINAL TABLE
//! and RETURNING.

use turborel::db::Database;

use super::Client;

/// Verifies: [EXTENSION] [DESIGN] RULES.md R-DOC-8, D185, D187: the
/// examples of row locks and advisory locks run as printed, as ANN.
#[test]
fn the_lock_examples_run_as_printed() {
    let db = Database::new();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/24-locks.md") > 0, "it shows its examples' output");
}

/// Verifies: [EXTENSION] [DESIGN] RULES.md R-DOC-8, D186: the examples of
/// LOCK TABLE run as printed, as ANN.
#[test]
fn the_lock_table_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/lock-table.md") > 0, "it shows its examples' output");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6] [DESIGN] RULES.md R-DOC-8, D183,
/// D184: the examples of delta tables and RETURNING run as printed, as
/// ANN.
#[test]
fn the_delta_table_examples_run_as_printed() {
    let db = Database::new();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/25-delta-tables.md") > 0, "it shows its examples' output");
}
