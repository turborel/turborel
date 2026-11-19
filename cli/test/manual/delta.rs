//! The examples of data change delta tables, run as printed: OLD, NEW and
//! FINAL TABLE.

use turborel::db::Database;

use super::Client;

/// Verifies: [ISO/IEC 9075-2:2023 §7.6] [DESIGN] RULES.md R-DOC-8, D174,
/// D175: the examples of delta tables run as printed, as ANN.
#[test]
fn the_delta_table_examples_run_as_printed() {
    let db = Database::new();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/24-delta-tables.md") > 0, "it shows its examples' output");
}
