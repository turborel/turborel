//! The examples of the Information Schema's short name views, run as
//! printed.

use turborel::db::Database;

use super::Client;
use crate::common::Scratch;

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.77] [DESIGN] RULES.md
/// R-DOC-8, D37: the examples of the short name views run as printed, as
/// ANN, in a database of ANN's schema alone.
#[test]
fn the_short_name_view_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let scratch = Scratch::new("manual-short-names");
    let section = ann.section("sql/07-information-schema.md", "## The short name views", &scratch);
    assert!(section > 0, "the section shows its examples");
}
