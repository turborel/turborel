//! How fast the file log commits (RULES.md D25): committers that each append
//! a record and sync it, on several threads, and how many syncs group
//! commit saved; and appends without syncs. Run with `cargo xtask bench`,
//! on the harness (RULES.md R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::sync::Arc;

use harness::{Bench, Case, RUNS, crc, on_threads};
use turborel::disk::io::{Durability, OsFileSystem};
use turborel::log::{FileLog, FileLogOptions, Log};

/// Committer thread counts, and commits each makes per run.
const COMMITTERS: [usize; 3] = [1, 4, 16];
const COMMITS_EACH: usize = 100;
/// Appends without syncs per run, and the record size.
const APPENDS: u64 = 200_000;
const RECORD: usize = 128;

fn main() {
    let bench = Bench::new("log");
    let record = vec![1_u8; RECORD];
    for threads in COMMITTERS {
        let options = FileLogOptions { durability: Durability::Data, ..FileLogOptions::default() };
        let log = FileLog::create(Arc::new(OsFileSystem), &bench.path(&format!("commit-{threads}")), crc(), options).unwrap();
        bench.measure(Case::new("append + sync (commit)", "commits").threads(threads).bytes(RECORD), || {
            on_threads(threads, |_| {
                for _ in 0..COMMITS_EACH {
                    let at = log.append(&record).unwrap();
                    log.sync(at).unwrap();
                }
            });
            (threads * COMMITS_EACH) as u64
        });
        // Every run, the warm-up's too, committed on this log.
        let commits = (RUNS + 1) * threads * COMMITS_EACH;
        harness::note(&format!("syncs per commit: {:.2}", log.counts().syncs as f64 / commits as f64));
    }
    let log = FileLog::create(Arc::new(OsFileSystem), &bench.path("appends"), crc(), FileLogOptions::default()).unwrap();
    bench.measure(Case::new("append", "records").bytes(RECORD), || {
        for _ in 0..APPENDS {
            log.append(&record).unwrap();
        }
        APPENDS
    });
}
