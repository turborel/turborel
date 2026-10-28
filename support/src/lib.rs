//! Fixtures shared by the tests of turborel's crates (RULES.md R-TEST-10):
//! a fixture used by the tests of more than one crate is written here,
//! once. Not part of the product: only tests depend on it.
//!
//! A panic is how a test reports failure, so this code may unwrap and
//! panic (RULES.md R-TEST-4).
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_panics_doc,
    reason = "test code: a panic is a test failure (RULES.md R-TEST-4)"
)]

use std::fs;
use std::path::PathBuf;

/// A directory of its own for one test, removed when the test ends, even
/// by a panic. Named after the test and the process, so tests running at
/// once never share one.
pub struct Scratch {
    /// The directory.
    pub dir: PathBuf,
}

impl Scratch {
    /// A new, empty directory for the test `test`.
    pub fn new(test: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("turborel-{test}-{}", std::process::id()));
        // A directory left by an earlier, interrupted run is replaced.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch { dir }
    }

    /// The path of `name` in the directory.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Cleanup only: a directory left behind is replaced by the next run.
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "tls")]
pub mod tls;

/// Helpers for what a server's client receives.
#[cfg(feature = "client")]
pub mod client {
    use turborel_client::Failure;
    use turborel_client::connection::Outcome;

    /// A query's rows as strings, each its values joined by commas.
    pub fn rows(outcome: &Outcome) -> Vec<String> {
        let Outcome::Rows { rows, .. } = outcome else { panic!("no rows: {outcome:?}") };
        rows.iter().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")).collect()
    }

    /// The SQLCODE a statement completed with.
    pub fn done(outcome: &Outcome) -> i32 {
        let Outcome::Done(completed) = outcome else { panic!("rows: {outcome:?}") };
        completed.sqlcode
    }

    /// The SQLSTATE and SQLCODE of `failure`.
    pub fn state(failure: &Failure) -> (&str, i32) {
        (failure.sqlstate.as_str(), failure.sqlcode)
    }
}
