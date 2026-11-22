//! Every case a run measures, in order: the TPROC-C-style workload with
//! each procedure set named, then each sysbench-style script named, each at
//! each thread count, each on a database of its own, loaded anew, as the
//! measurement box restores its data for each run, and removed after.

use std::path::Path;
use std::time::Instant;

use turborel::db::Database;

use crate::client::{Connect, InProcess};
use crate::harness::{Bench, note};
use crate::settings::Settings;
use crate::{sysbench, sysbench_run, tpcc_run};

/// How the terminals reach a database: in this process, or through a server.
pub trait Serve {
    /// What makes their clients, kept while the case runs.
    type Served: Connect;

    /// The terminals' way to `db`.
    fn serve(&self, db: Database) -> Self::Served;
}

/// Terminals in this process, each a session.
pub struct Embedded;

impl Serve for Embedded {
    type Served = InProcess;

    fn serve(&self, db: Database) -> InProcess {
        InProcess(db)
    }
}

/// A new database in `dir`, built by `build`, its load timed.
fn built(settings: &Settings, dir: &Path, build: impl FnOnce(&Database)) -> Database {
    let db = settings.store.open(dir);
    let loading = Instant::now();
    build(&db);
    note(&format!("loaded in {:.1} s", loading.elapsed().as_secs_f64()));
    db
}

/// Measures every case `settings` names, its terminals reaching each
/// database as `serve` says.
pub fn run(bench: &Bench, settings: &Settings, serve: &impl Serve) {
    if settings.runs("tpcc") {
        for set in &settings.procedures {
            for &threads in &settings.threads {
                let dir = bench.path(&format!("tpcc-{set}-{threads}"));
                let db = built(settings, &dir, |db| tpcc_run::build(db, set));
                tpcc_run::measure(bench, &serve.serve(db), (set, threads), settings.secs);
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
    for script in sysbench::SCRIPTS.iter().filter(|s| settings.runs(s.name)) {
        for &threads in &settings.threads {
            let dir = bench.path(&format!("{}-{threads}", script.name));
            let db = built(settings, &dir, |db| sysbench_run::build(db, settings.table_size));
            sysbench_run::measure(bench, &serve.serve(db), (script, threads, settings.table_size), settings.secs);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
