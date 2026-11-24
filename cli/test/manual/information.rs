//! The examples of the Information Schema's views of what depends on
//! what, of its short name views and of its column statistics, run as
//! printed.

use std::sync::Arc;

use turborel::clock::{Clock, FixedClock};
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

/// The time the column statistics examples' clock shows, 2026-11-20
/// 12:00:00 UTC, in microseconds from the epoch.
const GATHERED_AT: i64 = 1_795_176_000_000_000;

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1] [DESIGN] RULES.md
/// R-DOC-8, D990, D991: the examples of COLUMN_STATISTICS run as printed,
/// as ANN, PART's statistics gathered after the first, as the chapter
/// says the maintenance has, the clock standing at `GATHERED_AT`.
#[test]
fn the_column_statistics_examples_run_as_printed() {
    let page = "sql/07-information-schema.md";
    let text = super::text_of(page);
    let section = text.split("\n## Column statistics\n").nth(1).and_then(|s| s.split("\n## ").next()).unwrap();
    let examples = super::examples_in(section);
    let db = Database::new();
    db.set_clock(Arc::new(FixedClock::at(GATHERED_AT)) as Arc<dyn Clock>).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let (first, rest) = examples.split_at(1);
    ann.run(page, first, None);
    db.analyze("ANN.PART").unwrap();
    assert_eq!(ann.run(page, rest, None), 1, "{page} shows its examples' output");
}
