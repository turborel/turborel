//! The manual's examples of partitioned tables (`doc/`), run as printed.

use turborel::db::Database;

use super::Client;

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D153 to D158: the chapter of
/// partitioned tables runs as printed, as ANN.
#[test]
fn the_partitioning_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/23-partitioning.md") > 0, "the chapter shows its examples' output");
}
