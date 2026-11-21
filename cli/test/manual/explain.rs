//! The manual's examples of EXPLAIN (`doc/`), run as printed.

use turborel::db::Database;

use super::{Client, examples};

/// Verifies: [DESIGN] RULES.md R-DOC-8, D376: the examples of EXPLAIN and
/// EXPLAIN ANALYZE run as printed, as ANN, the client showing its plan's
/// lines as rows, the database's clock standing still, so no time is
/// shown (R-TEST-5). A plan's estimates read the statistics the
/// database's maintenance gathers in the background once planning finds
/// them due (RULES.md D31, D35), so each example's maintenance is done
/// before the next runs, as an idle machine's job has done it: on a
/// loaded one the job may still be waiting for a thread, and a plan be
/// estimated without them.
#[test]
fn the_explain_examples_run_as_printed() {
    let db = Database::new();
    db.set_clock(std::sync::Arc::new(turborel::clock::FixedClock::default())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let page = "sql/statements/explain.md";
    let all = examples(page);
    for (n, example) in all.iter().enumerate() {
        ann.run(&format!("{page}, from example {n}"), std::slice::from_ref(example), None);
        db.maintain().unwrap();
    }
    assert!(!all.is_empty(), "it shows its examples' output");
}
