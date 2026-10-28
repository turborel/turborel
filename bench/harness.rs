//! The harness every benchmark in `bench/` runs on (RULES.md R-BENCH-1).
//! A benchmark file holds its workloads; this module times them, reports
//! them, and provides what they share.
//!
//! Each measurement runs its workload once untimed, to warm caches and
//! files, and then `RUNS` times. It reports the median rate, the spread of
//! the runs around it, MiB/s when each unit of work moves a known number
//! of bytes, and, at more than one thread, the speedup over the same
//! operation on one. With `OUT_VAR` set, as `cargo xtask bench` sets it, it
//! also appends each result to that file, which `cargo xtask bench --save`
//! keeps and `--compare` compares against.
//!
//! Files go to a directory of the system's temporary directory, removed
//! when the benchmark ends. A benchmark measures, it does not test: timings
//! depend on the machine, so it has no assertions on them, and it is not
//! part of `verify` (RULES.md D6).

#![expect(clippy::print_stdout, reason = "a benchmark reports by printing (RULES.md R-DEV-3)")]
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    reason = "benchmark code: each benchmark uses part of the harness, a failure ends the run, and the figures are estimates (RULES.md R-DEV-3)"
)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use turborel::disk::buffer::{BufferPool, Clock};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::OsFileSystem;
use turborel::disk::pager::{DEFAULT_PAGE_SIZE, Pager};
use turborel::hash::HashFunction;
use turborel::hash::crc::{CRC32_ISCSI, Crc};

/// Timed runs of each measurement, after one untimed warm-up.
pub const RUNS: usize = 5;
/// The file each result is appended to, if set (`cargo xtask bench`).
pub const OUT_VAR: &str = "TURBOREL_BENCH_OUT";
/// A prime, coprime to every count the benchmarks use, so `scatter` visits
/// each of `0..count` once, out of order.
pub const SCATTER: u64 = 7919;
/// Bytes in a mebibyte, for the figures.
const MIB: f64 = 1024.0 * 1024.0;

/// What a measurement measures: an operation, the unit its rate counts,
/// the threads it runs on, and the bytes each unit moves (0 if none).
/// These identify its result, which `cargo xtask bench --compare` matches
/// with a saved one, so a benchmark names each operation once.
pub struct Case {
    operation: String,
    unit: &'static str,
    threads: usize,
    bytes: usize,
}

impl Case {
    /// `operation`, counted in `unit`s, on one thread.
    pub fn new(operation: impl Into<String>, unit: &'static str) -> Case {
        Case { operation: operation.into(), unit, threads: 1, bytes: 0 }
    }

    /// On `threads` threads.
    pub fn threads(self, threads: usize) -> Case {
        Case { threads, ..self }
    }

    /// Each unit moves `bytes` bytes: report MiB/s too.
    pub fn bytes(self, bytes: usize) -> Case {
        Case { bytes, ..self }
    }
}

/// One benchmark: its name, its directory, and what it has measured.
pub struct Bench {
    name: &'static str,
    dir: PathBuf,
    out: RefCell<Option<File>>,
    /// The median rate of each operation (and byte size) on one thread.
    single: RefCell<HashMap<(String, usize), f64>>,
    /// What it has measured, so no result is reported twice.
    measured: RefCell<HashSet<(String, usize, usize)>>,
}

impl Bench {
    /// The benchmark `name`, with a new directory, its header printed.
    pub fn new(name: &'static str) -> Bench {
        let dir = env::temp_dir().join(format!("turborel-bench-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let out = env::var_os(OUT_VAR).map(|path| OpenOptions::new().create(true).append(true).open(path).unwrap());
        println!("\n== {name}");
        println!(
            "{:<36} {:>7} {:>8} {:>14} {:<12} {:>7} {:>10} {:>8}",
            "operation", "threads", "bytes", "median", "", "spread", "MiB/s", "speedup"
        );
        Bench { name, dir, out: RefCell::new(out), single: RefCell::new(HashMap::new()), measured: RefCell::new(HashSet::new()) }
    }

    /// The path of `file` in the benchmark's directory.
    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// The path of `file`, with any file there removed, for a workload that
    /// creates it anew on each run.
    pub fn fresh(&self, file: &str) -> PathBuf {
        let path = self.path(file);
        let _ = fs::remove_file(&path);
        path
    }

    /// A new pager file `file`, default-size pages, checksummed with CRC-32C.
    pub fn pager(&self, file: &str) -> Pager {
        Pager::create(&OsFileSystem, &self.fresh(file), crc(), Pipeline::none(), DEFAULT_PAGE_SIZE).unwrap()
    }

    /// Measures `run`, which does the same work each time it is called and
    /// returns how many units it did.
    pub fn measure(&self, case: Case, mut run: impl FnMut() -> u64) {
        self.measure_fresh(case, || (), |()| run());
    }

    /// Measures `run` on a state `setup` makes anew, untimed, for each run:
    /// for work that changes what it works on.
    ///
    /// # Panics
    ///
    /// The benchmark measured the same case before.
    pub fn measure_fresh<S>(&self, case: Case, mut setup: impl FnMut() -> S, mut run: impl FnMut(S) -> u64) {
        let key = (case.operation.clone(), case.threads, case.bytes);
        assert!(self.measured.borrow_mut().insert(key), "{} measures {:?} twice: results are matched by name", self.name, case.operation);
        let mut rates = Vec::with_capacity(RUNS);
        for timed in [false].into_iter().chain([true; RUNS]) {
            let state = setup();
            let started = Instant::now();
            let units = run(state);
            let secs = started.elapsed().as_secs_f64();
            if timed {
                rates.push(units as f64 / secs);
            }
        }
        rates.sort_by(f64::total_cmp);
        self.report(case, Rates { median: rates[RUNS / 2], low: rates[0], high: rates[RUNS - 1] });
    }

    /// Prints a result, and appends it to the results file if there is one.
    fn report(&self, case: Case, rates: Rates) {
        let Case { operation, unit, threads, bytes } = case;
        let speedup = if threads == 1 {
            String::new()
        } else {
            let one = self.single.borrow().get(&(operation.clone(), bytes)).copied();
            one.map_or_else(String::new, |one| format!("{:.2}x", rates.median / one))
        };
        let spread = (rates.high - rates.low) / 2.0_f64 / rates.median * 100.0_f64;
        let (size, mib) = if bytes == 0 {
            (String::new(), String::new())
        } else {
            (bytes.to_string(), format!("{:.1}", rates.median * bytes as f64 / MIB))
        };
        let rate = format!("{unit}/s");
        println!("{operation:<36} {threads:>7} {size:>8} {:>14.0} {rate:<12} {spread:>6.1}% {mib:>10} {speedup:>8}", rates.median);
        if let Some(out) = self.out.borrow_mut().as_mut() {
            let (name, median, low, high) = (self.name, rates.median, rates.low, rates.high);
            writeln!(out, "{name}\t{operation}\t{threads}\t{bytes}\t{unit}\t{median:.3}\t{low:.3}\t{high:.3}").unwrap();
        }
        if threads == 1 {
            self.single.borrow_mut().insert((operation, bytes), rates.median);
        }
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Prints a figure about the measurement before, which is not a rate.
pub fn note(text: &str) {
    println!("   {text}");
}

/// The median, lowest and highest rates of a measurement's runs.
#[derive(Clone, Copy)]
struct Rates {
    median: f64,
    low: f64,
    high: f64,
}

/// Runs `work(t)` for `t` in `0..threads`, each on a thread of its own, and
/// waits for all.
pub fn on_threads(threads: usize, work: impl Fn(usize) + Sync) {
    thread::scope(|s| {
        for t in 0..threads {
            let work = &work;
            s.spawn(move || work(t));
        }
    });
}

/// The `n`th of `0..count` in a scattered order: each once, for `n` in
/// `0..count`.
pub fn scatter(n: u64, count: u64) -> u64 {
    n * SCATTER % count
}

/// CRC-32C, the checksum the benchmarks' files use.
pub fn crc() -> Arc<dyn HashFunction> {
    Arc::new(Crc::new(CRC32_ISCSI).unwrap())
}

/// A buffer pool of `frames` frames, with clock eviction.
pub fn pool(frames: usize) -> Arc<BufferPool> {
    Arc::new(BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap())
}
