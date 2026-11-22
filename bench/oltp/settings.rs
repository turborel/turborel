//! What a run measures, from environment variables, so one case can run
//! alone, under a profiler as well:
//!
//! - `TURBOREL_OLTP_WORKLOADS`, by commas, of `tpcc` and the sysbench-style
//!   workloads' names; all if unset.
//! - `TURBOREL_OLTP_THREADS`, by commas; 1, 8 and 32 if unset.
//! - `TURBOREL_OLTP_SECS`, the seconds of each timed run, five of them after
//!   one untimed; 2 if unset.
//! - `TURBOREL_OLTP_PROCEDURES`, `old`, `new` or both by a comma, the
//!   TPROC-C-style procedure sets; both if unset.
//! - `TURBOREL_OLTP_SYNC`, `off` for commits that do not wait for the log's
//!   sync; on if unset.
//! - `TURBOREL_OLTP_TABLE_SIZE`, the rows of each sysbench-style table;
//!   100,000 if unset.
//! - `TURBOREL_OLTP_POOL_MB`, the buffer pool's mebibytes; 4,096 if unset.
//! - `TURBOREL_OLTP_MODE`, `embedded` or `protocol`, where the protocol
//!   mode's example is built; embedded if unset.

use std::env;
use std::time::Duration;

use crate::store::Store;

/// The run's settings.
pub struct Settings {
    pub workloads: Option<Vec<String>>,
    pub threads: Vec<usize>,
    pub secs: Duration,
    pub procedures: Vec<String>,
    pub store: Store,
    pub table_size: i64,
    pub mode: Mode,
}

/// Where the terminals' sessions are.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// In this process, each a `Session`.
    Embedded,
    /// Across the native protocol on loopback, to a server in this process.
    Protocol,
}

/// The thread counts of the measurement box's runs.
const THREADS: [usize; 3] = [1, 8, 32];
/// The default seconds of each timed run.
const SECS: u64 = 2;
/// The rows of each sysbench-style table on the measurement box.
const TABLE_SIZE: i64 = 100_000;
/// The default buffer pool, in mebibytes: every table fits.
const POOL_MB: i128 = 4_096;

fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

fn list(name: &str) -> Option<Vec<String>> {
    var(name).map(|v| v.split(',').map(|s| s.trim().to_owned()).collect())
}

impl Settings {
    /// The settings the environment gives.
    pub fn from_env() -> Settings {
        let threads = list("TURBOREL_OLTP_THREADS").map_or(THREADS.to_vec(), |l| l.iter().map(|t| t.parse().unwrap()).collect());
        let secs = Duration::from_secs_f64(var("TURBOREL_OLTP_SECS").map_or(SECS as f64, |s| s.parse().unwrap()));
        let procedures = list("TURBOREL_OLTP_PROCEDURES").unwrap_or_else(|| vec!["old".to_owned(), "new".to_owned()]);
        let sync = var("TURBOREL_OLTP_SYNC").is_none_or(|s| !s.eq_ignore_ascii_case("off"));
        let pool_mb: i128 = var("TURBOREL_OLTP_POOL_MB").map_or(POOL_MB, |s| s.parse().unwrap());
        let table_size = var("TURBOREL_OLTP_TABLE_SIZE").map_or(TABLE_SIZE, |s| s.parse().unwrap());
        let mode = match var("TURBOREL_OLTP_MODE").as_deref() {
            Some("protocol") => Mode::Protocol,
            Some("embedded") | None => Mode::Embedded,
            Some(other) => panic!("TURBOREL_OLTP_MODE is embedded or protocol, not {other}"),
        };
        let store = Store { sync, pool_bytes: pool_mb << 20 };
        Settings { workloads: list("TURBOREL_OLTP_WORKLOADS"), threads, secs, procedures, store, table_size, mode }
    }

    /// Whether workload `name` is to run.
    pub fn runs(&self, name: &str) -> bool {
        self.workloads.as_ref().is_none_or(|w| w.iter().any(|n| n == name))
    }
}
