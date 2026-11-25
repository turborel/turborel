//! \[DESIGN\] A long load within the process's limits (RULES.md D1136 to
//! D1138): the log let go of by checkpoints that come due as it grows, so
//! its segments, and their descriptors, do not pile up; and the rows an
//! execution of a load is given held, never spilled, so a batch larger
//! than a work area opens no spill file.

use std::path::Path;
use std::thread;
use std::time::Duration;

use turborel::db::{Database, OpenOptions};
use turborel::params::Load;
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, number, os};
use crate::faults::{FaultyFs, Op, Rule};

/// The segments of the log in `dir`.
fn segments(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("log")).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("segment")).count()
}

/// Whether `db` takes at least `n` checkpoints within a few seconds.
fn checkpointed(db: &Database, n: u64) -> bool {
    (0..500).any(|_| {
        let taken = db.checkpoints_taken() >= n;
        if !taken {
            thread::sleep(Duration::from_millis(10));
        }
        taken
    })
}

/// Verifies: [DESIGN] RULES.md D1137: with CHECKPOINT.AFTER_CHANGES far
/// off and nothing written to the side file, CHECKPOINT.AFTER_BYTES
/// written to the log bring a checkpoint, which lets the log's segments
/// go; with neither due, none is taken and the segments stay.
#[test]
fn a_log_grown_past_after_bytes_brings_a_checkpoint() {
    let scratch = Scratch::new("load-limits-log");
    for (bytes, due) in [(256 * 1024, true), (i64::MAX, false)] {
        let dir = scratch.path(&format!("db-{due}"));
        let db = Database::new();
        db.set_setting("LOG.SEGMENT_BYTES", &number(16 * 1024)).unwrap();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
        db.set_setting("CHECKPOINT.AFTER_BYTES", &number(bytes)).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION W CREATE TABLE W (S CHAR(240))").unwrap();
        let s = db.session("W");
        // About 4 MB of log, in 40 transactions.
        for n in 0..8_000 {
            s.run(&format!("INSERT INTO W VALUES ('{n:x>240}')")).unwrap();
            if n % 200 == 199 {
                s.commit().unwrap();
            }
        }
        assert_eq!(checkpointed(&db, 1), due, "after bytes {bytes}");
        let kept = segments(&dir);
        assert!(if due { kept < 40 } else { kept > 100 }, "after bytes {bytes}: {kept} segments");
        drop(s);
        db.close().unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D1138: an execution of a load given rows
/// of more bytes than any work area holds, MEMORY.WORK_BYTES set low,
/// inserts them all and opens no spill file, of each store.
#[test]
fn a_load_batch_past_work_memory_opens_no_spill_file() {
    for using in ["", "USING columnstore"] {
        let scratch = Scratch::new("load-limits-spill");
        let (fs, faults) = FaultyFs::with_faults();
        let db = Database::new();
        db.open_files(&fs, &scratch.path("db"), &OpenOptions::default()).unwrap();
        db.set_setting("MEMORY.WORK_BYTES", &number(64 * 1024)).unwrap();
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, S CHAR(500)) {using}")).unwrap();
        faults.inject(Rule::fail(Op::Open).on(".spill"));
        let s = db.session("S");
        let p = s.prepare("INSERT INTO T (K, S) VALUES (?, ?)").unwrap();
        // About 6 MB, past the most rows held apart from operators hold.
        let rows: Vec<Vec<Value>> =
            (0..12_000_i64).map(|k| vec![Value::Exact(Decimal::new(i128::from(k), 0)), Value::Char(format!("{k:y>500}").into())]).collect();
        s.execute_prepared_rows(&p, &rows, Load::Ending).unwrap();
        s.commit().unwrap();
        assert_eq!(crate::common::count(&s, "SELECT K FROM T"), 12_000, "{using}");
        assert_eq!(faults.injected(), 0, "{using}: no spill file opened");
        drop(s);
    }
}
