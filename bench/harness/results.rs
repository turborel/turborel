//! Measurements of work whose result matters as well as its speed, such as
//! a query's: the harness times the runs as it times any (`harness.rs`),
//! hands back how long each took and what it gave, and keeps a line of
//! what the result was beside the rates, so `cargo xtask bench --compare`
//! can tell a build that answers differently from one that is only faster
//! or slower.

use std::io::Write;
use std::time::Instant;

use super::{Bench, Case, Rates};

/// Marks the line of a results file that holds what a measurement's work
/// gave, as `cargo xtask bench` reads it.
const RESULT: &str = "@result";

/// What the timed runs of a measurement took and gave.
pub struct Timed<T> {
    /// The seconds of each timed run, in the order they ran.
    pub secs: Vec<f64>,
    /// What each timed run gave.
    pub gave: Vec<T>,
}

impl<T> Timed<T> {
    /// The seconds of its fastest run.
    pub fn least(&self) -> f64 {
        self.secs.iter().copied().fold(f64::INFINITY, f64::min)
    }
}

impl Bench {
    /// Measures `run` once untimed and then `runs` times, reports the rate
    /// of runs a second as any measurement's, and hands back each timed
    /// run's seconds and what it gave.
    ///
    /// # Panics
    ///
    /// The benchmark measured the same case before, or `runs` is 0.
    pub fn measure_timed<T>(&self, case: Case, runs: usize, mut run: impl FnMut() -> T) -> Timed<T> {
        let key = (case.operation.clone(), case.threads, case.bytes);
        assert!(self.measured.borrow_mut().insert(key), "{} measures {:?} twice: results are matched by name", self.name, case.operation);
        drop(run());
        let mut timed = Timed { secs: Vec::with_capacity(runs), gave: Vec::with_capacity(runs) };
        for _ in 0..runs {
            let started = Instant::now();
            let gave = run();
            timed.secs.push(started.elapsed().as_secs_f64());
            timed.gave.push(gave);
        }
        let mut rates: Vec<f64> = timed.secs.iter().map(|secs| 1.0_f64 / secs).collect();
        rates.sort_by(f64::total_cmp);
        self.report(case, Rates { median: rates[runs / 2], low: rates[0], high: rates[runs - 1] });
        timed
    }

    /// Keeps `result`, what the work of `operation` on `threads` threads
    /// gave, in the results file if there is one, for a later run to be
    /// compared with.
    pub fn keep_result(&self, operation: &str, threads: usize, result: &str) {
        if let Some(out) = self.out.borrow_mut().as_mut() {
            writeln!(out, "{RESULT}\t{}\t{operation}\t{threads}\t{result}", self.name).unwrap();
        }
    }
}
