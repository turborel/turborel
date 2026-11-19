//! How fast the server's transport is (RULES.md D42, D45): over loopback
//! TCP, the local transport (a Unix socket, or a named pipe on
//! Windows) and TLS, the round trip of a trivial query, many
//! small queries with and without pipelining, a large result streamed,
//! bulk inserts through prepared statements, the setup of a connection,
//! plain and TLS, trusted and by SCRAM, and hundreds of connections at
//! once. Each server runs in this process on a runtime of its own, as
//! `turboreld`'s, over an in-memory database, so the figures are the
//! transport's and the protocol's, not a disk's.
//!
//! The benchmarks of `bench/` are targets of the core crate, which
//! depends on nothing (RULES.md D3), so one that needs the server and the
//! client is an example of the server's crate, run on the same harness
//! (RULES.md R-BENCH-1): `cargo xtask bench network`. Its results are
//! kept and compared as every benchmark's are.
//! `TURBOREL_NETWORK_ONLY=stream,small` runs those workloads alone.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::print_stdout,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

// The harness's own modules sit beside it, in `bench/harness/`.
#[path = "../../../bench/"]
mod benches {
    pub mod harness;
}
mod local;
mod raw;
mod served;
mod workloads;

use benches::harness;
use harness::Bench;

/// Names the workloads to run, by commas, as `stream,small`; all if unset.
const ONLY_VAR: &str = "TURBOREL_NETWORK_ONLY";

/// A workload.
type Workload = fn(&Bench, &served::Served);

/// The workloads, by name.
const WORKLOADS: [(&str, Workload); 6] = [
    ("latency", workloads::latency),
    ("small", workloads::small),
    ("stream", workloads::streaming),
    ("inserts", workloads::inserts),
    ("setup", workloads::setup),
    ("concurrent", workloads::concurrent),
];

fn main() {
    let only = std::env::var(ONLY_VAR).ok();
    let bench = Bench::new("network");
    let served = served::Served::start();
    for (name, workload) in WORKLOADS {
        if only.as_deref().is_none_or(|only| only.split(',').any(|o| o == name)) {
            workload(&bench, &served);
        }
    }
}
