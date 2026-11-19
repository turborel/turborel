//! The examples of the statements of backups and of the Operator's
//! Guide's chapter on them, run as printed.

use std::sync::Arc;

use turborel::clock::{Clock, FixedClock};
use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};

use super::Client;
use crate::common::Scratch;

/// The time the examples' clock shows, 2026-11-19 03:00:00 UTC, in
/// microseconds from the epoch.
const SHOWN_AT: i64 = 1_795_057_200_000_000;

/// A database kept in `scratch`, its clock fixed at `SHOWN_AT`, whose
/// owner is ANN, beside the user BOB.
fn kept(scratch: &Scratch) -> Database {
    let db = Database::new();
    db.set_clock(Arc::new(FixedClock::at(SHOWN_AT)) as Arc<dyn Clock>).unwrap();
    db.open_files(&(Arc::new(OsFileSystem) as Arc<dyn FileSystem>), &scratch.path("db"), &OpenOptions::default()).unwrap();
    db.create_user("ANN", b"x").unwrap();
    db.create_user("BOB", b"x").unwrap();
    db.set_owner("ANN").unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D160, D164, D166: the examples
/// of BACKUP DATABASE, CREATE RESTORE POINT and RESCAN BACKUP TARGET run
/// as printed, by the database's owner, ANN, in one database kept in a
/// directory.
#[test]
fn the_backup_statement_examples_run_as_printed() {
    let scratch = Scratch::new("manual-backups");
    let db = kept(&scratch);
    let mut ann = Client::of(&db, "ANN");
    for page in ["sql/statements/backup-database.md", "sql/statements/create-restore-point.md", "sql/statements/rescan-backup-target.md"] {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D160, D163, D164: the
/// Operator's Guide's examples of backups, the archive and restore points
/// run as printed, by the database's owner, ANN.
#[test]
fn the_backup_chapter_runs_as_printed() {
    let scratch = Scratch::new("manual-backup-chapter");
    let db = kept(&scratch);
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("operator/10-backups.md") > 0, "it shows its examples' output");
}
