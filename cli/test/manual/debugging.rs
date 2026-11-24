//! The manual's examples of the routine debugger and its DEBUG privilege
//! (`doc/`), run as printed.

use turborel::db::Database;

use super::Client;

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D1000 to D1002: the examples of
/// GRANT DEBUG run as printed, as ANN.
#[test]
fn the_grant_debug_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/grant-debug.md") > 0, "it shows its examples' output");
}
