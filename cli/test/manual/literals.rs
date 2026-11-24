//! The manual's examples of literals (`doc/sql/01-lexical.md`), run as
//! printed.

use turborel::db::Database;

use super::Client;
use crate::common::Scratch;

/// Verifies: [ISO/IEC 9075-2:2023 §5.3 SR29, SR30, §6.13 GR8a ii] RULES.md
/// R-DOC-8, D970, D971: the Lexical Elements chapter's section of
/// literals runs as printed, as ANN: a literal of 39 digits or past
/// DOUBLE PRECISION's range is SQLSTATE 42000 and SQLCODE -103, as the
/// client shows it, and its text cast is 22003.
#[test]
fn the_literal_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let scratch = Scratch::new("manual-literals");
    assert!(ann.section("sql/01-lexical.md", "## Literals", &scratch) > 0, "the section shows its examples");
}
