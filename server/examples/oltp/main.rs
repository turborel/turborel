//! The OLTP workloads of `bench/oltp.rs`, through the native protocol: the
//! same cases, each database served by a server in this process, as
//! `turboreld` serves it, and each terminal a connection of its own over
//! loopback TCP, so the protocol's and the network's cost is added back.
//! `TURBOREL_OLTP_MODE=embedded` runs them in this process instead, as the
//! core's bench does.
//!
//! The benchmarks of `bench/` are targets of the core crate, which depends
//! on nothing (RULES.md D3), so one that needs the server and the client is
//! an example of the server's crate, on the same harness (RULES.md
//! R-BENCH-1): `TURBOREL_OLTP_MODE=protocol cargo run --release -p
//! turborel-server --example oltp`, with the settings of `bench/oltp/settings.rs`.

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
    clippy::print_stdout,
    missing_docs,
    reason = "benchmark code: a failure ends the run, and the figures are estimates (RULES.md R-DEV-3)"
)]

// The harness's own modules sit beside it, in `bench/harness/`.
#[path = "../../../bench/"]
mod benches {
    pub mod harness;
}
#[path = "../../../bench/oltp/cases.rs"]
mod cases;
#[path = "../../../bench/oltp/client.rs"]
mod client;
#[path = "../../../bench/oltp/measure.rs"]
mod measure;
#[path = "../../../bench/oltp/random.rs"]
mod random;
mod served;
#[path = "../../../bench/oltp/settings.rs"]
mod settings;
#[path = "../../../bench/oltp/store.rs"]
mod store;
#[path = "../../../bench/oltp/sysbench.rs"]
mod sysbench;
#[path = "../../../bench/oltp/sysbench_load.rs"]
mod sysbench_load;
#[path = "../../../bench/oltp/sysbench_run.rs"]
mod sysbench_run;
#[path = "../../../bench/oltp/tpcc.rs"]
mod tpcc;
#[path = "../../../bench/oltp/tpcc_load.rs"]
mod tpcc_load;
#[path = "../../../bench/oltp/tpcc_native.rs"]
mod tpcc_native;
#[path = "../../../bench/oltp/tpcc_procs.rs"]
mod tpcc_procs;
#[path = "../../../bench/oltp/tpcc_procs_new.rs"]
mod tpcc_procs_new;
#[path = "../../../bench/oltp/tpcc_run.rs"]
mod tpcc_run;
#[path = "../../../bench/oltp/tpcc_schema.rs"]
mod tpcc_schema;

use benches::harness;
use harness::Bench;
use settings::{Mode, Settings};

fn main() {
    let settings = Settings::from_env();
    let bench = Bench::new("oltp_protocol");
    match settings.mode {
        Mode::Protocol => cases::run(&bench, &settings, &served::Protocol),
        Mode::Embedded => cases::run(&bench, &settings, &cases::Embedded),
    }
}
