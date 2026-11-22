//! OLTP workloads in seconds, deterministic, in this process, so a change's
//! cost per transaction shows without a server, a network or a queue, and
//! a profiler can run on one case alone. A TPROC-C-style workload of one
//! warehouse runs its five transactions as stored procedures, in either of
//! two procedure sets (`oltp/tpcc.rs`), and eight sysbench-style workloads
//! run point queries, range queries, updates, deletes and inserts on ten
//! tables (`oltp/sysbench.rs`); each case runs on a database of its own, kept in
//! files and durable as a server keeps it. Each reports transactions per
//! second, on the harness (RULES.md R-BENCH-1), and the time of a
//! transaction (`oltp/measure.rs`). The settings are environment variables
//! (`oltp/settings.rs`): `TURBOREL_OLTP_WORKLOADS=tpcc
//! TURBOREL_OLTP_THREADS=8 TURBOREL_OLTP_PROCEDURES=new cargo bench --bench
//! oltp` runs one case.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::integer_division_remainder_used,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::modulo_arithmetic,
    clippy::float_arithmetic,
    missing_docs,
    reason = "benchmark code: a failure ends the run, and the figures are estimates (RULES.md R-DEV-3)"
)]

#[path = "oltp/cases.rs"]
mod cases;
#[path = "oltp/client.rs"]
mod client;
mod harness;
#[path = "oltp/measure.rs"]
mod measure;
#[path = "oltp/random.rs"]
mod random;
#[path = "oltp/settings.rs"]
mod settings;
#[path = "oltp/store.rs"]
mod store;
#[path = "oltp/sysbench.rs"]
mod sysbench;
#[path = "oltp/sysbench_load.rs"]
mod sysbench_load;
#[path = "oltp/sysbench_run.rs"]
mod sysbench_run;
#[path = "oltp/tpcc.rs"]
mod tpcc;
#[path = "oltp/tpcc_load.rs"]
mod tpcc_load;
#[path = "oltp/tpcc_native.rs"]
mod tpcc_native;
#[path = "oltp/tpcc_procs.rs"]
mod tpcc_procs;
#[path = "oltp/tpcc_procs_new.rs"]
mod tpcc_procs_new;
#[path = "oltp/tpcc_run.rs"]
mod tpcc_run;
#[path = "oltp/tpcc_schema.rs"]
mod tpcc_schema;

use harness::Bench;
use settings::{Mode, Settings};

fn main() {
    let settings = Settings::from_env();
    assert!(settings.mode == Mode::Embedded, "the protocol mode is the server's example oltp");
    cases::run(&Bench::new("oltp"), &settings, &cases::Embedded);
}
