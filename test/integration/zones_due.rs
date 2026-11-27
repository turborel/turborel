//! A table opened whose zones are due has none summed up until one is
//! asked for (RULES.md D1212). A database opened does no maintenance of a
//! table nothing reads, sums up the zones of one a scan asks, and keeps
//! what it summed up from one start to the next.

use std::path::Path;

use turborel::db::{Database, OpenOptions};
use turborel::threads::JobPolicy;

use crate::common::{Scratch, os, rows};

/// Rows of table R: three zones of row ids, the last in part.
const ROWS: i32 = 2_100;

/// The database in `dir`, its background work done only when a test asks.
fn open(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db
}

/// Whether maintenance did any work on R since the database was opened,
/// as TABLE_STATISTICS shows A.
fn worked(db: &Database) -> bool {
    let sql = "SELECT LAST_COMPACTED IS NULL FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'R'";
    rows(&db.session("A"), sql).unwrap() == ["FALSE"]
}

/// Has A scan R by a condition of a column its zones keep, which asks
/// them, and ends the transaction.
fn asked(db: &Database) {
    let a = db.session("A");
    assert_eq!(rows(&a, "SELECT K FROM R WHERE V = 7").unwrap(), ["7"]);
    a.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D1212, D868, D1163: a table whose zones
/// were not known when the database was closed is not summed up as it is
/// opened, nor while another table alone is read, maintenance doing no
/// work on it; a scan told a condition asks its zones, and maintenance then
/// sums them up; closed and opened again, it has none left to sum up,
/// though asked again. A regression test (R-TEST-7): it fails if a table
/// opened is summed up before a zone of it is asked for.
#[test]
fn a_table_opened_is_summed_up_only_once_its_zones_are_asked_for() {
    let scratch = Scratch::new("zones-due");
    let dir = scratch.path("db");
    {
        let db = open(&dir);
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (K INTEGER, V INTEGER) CREATE TABLE Q (K INTEGER)").unwrap();
        let a = db.session("A");
        let values: Vec<String> = (0..ROWS).map(|k| format!("({k}, {k})")).collect();
        a.run(&format!("INSERT INTO R VALUES {}", values.join(", "))).unwrap();
        a.run("INSERT INTO Q VALUES (1), (2)").unwrap();
        a.commit().unwrap();
        // Written cold, then asked: its zones are not known, and none is
        // summed up before the close.
        asked(&db);
        db.close().unwrap();
    }
    {
        let db = open(&dir);
        let a = db.session("A");
        assert_eq!(rows(&a, "SELECT K FROM Q WHERE K = 2").unwrap(), ["2"]);
        a.commit().unwrap();
        db.run_due().unwrap();
        assert!(!worked(&db), "never asked: no work on it");
        asked(&db);
        db.run_due().unwrap();
        assert!(worked(&db), "asked: summed up");
        db.maintain().unwrap();
        db.close().unwrap();
    }
    let db = open(&dir);
    asked(&db);
    db.run_due().unwrap();
    assert!(!worked(&db), "summed up before the close: none left");
}
