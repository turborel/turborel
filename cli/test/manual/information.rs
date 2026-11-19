//! The examples of the Information Schema's views of what depends on
//! what and of its short name views, run as printed.

use turborel::db::Database;

use super::Client;
use crate::common::Scratch;

/// Verifies: [ISO/IEC 9075-11:2006 draft §5.77, §5.44, §5.74, §5.17]
/// [DESIGN] RULES.md R-DOC-8, D37: the examples of the views of what
/// depends on what and of the short name views run as printed, as ANN, in
/// a database of ANN's schema alone.
#[test]
fn the_short_name_view_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let scratch = Scratch::new("manual-short-names");
    for heading in ["## What depends on what", "## The short name views"] {
        let section = ann.section("sql/07-information-schema.md", heading, &scratch);
        assert!(section > 0, "{heading} shows its examples");
    }
}
