//! The examples of the Operator's Guide's chapter on monitoring, run as
//! printed.

use std::sync::Arc;

use turborel::clock::{Clock, FixedClock};
use turborel::db::Database;

use super::Client;

/// The time the examples' clock shows, 2026-11-20 12:00:00 UTC, in
/// microseconds from the epoch.
const SHOWN_AT: i64 = 1_795_176_000_000_000;

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D520, D521: the examples of
/// Monitoring run as printed, by the database's owner, ANN, in a schema
/// of its own, beside the user BOB, the clock standing at `SHOWN_AT`.
#[test]
fn the_monitoring_examples_run_as_printed() {
    let db = Database::new();
    db.set_clock(Arc::new(FixedClock::at(SHOWN_AT)) as Arc<dyn Clock>).unwrap();
    db.create_user("ANN", b"x").unwrap();
    db.create_user("BOB", b"x").unwrap();
    db.set_owner("ANN").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("operator/11-monitoring.md") > 0, "it shows its examples' output");
}
