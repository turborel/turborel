//! The manual's examples of UUIDs (`doc/`), run as printed.

use turborel::db::Database;

use super::Client;
use crate::common::Scratch;

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D180, D181: the Data Types
/// chapter's section of UUIDs runs as printed, as ANN.
#[test]
fn the_uuid_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let scratch = Scratch::new("manual-uuids");
    assert!(ann.section("sql/02-data-types.md", "## UUIDs", &scratch) > 0, "the section shows its examples");
}
