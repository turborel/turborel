//! Fresh databases, complete operation samples and untimed final-state
//! checks for mixed read/write benchmarks (RULES.md R-BENCH-1).

use std::io::Write;
use std::time::{Duration, Instant};

use super::{Bench, Case, RUNS, Rates, note};

/// Complete per-operation latencies, separated by operation type.
#[derive(Default)]
pub struct Samples {
    /// Read, including fetch and commit.
    pub reads: Vec<Duration>,
    /// Update, including commit.
    pub writes: Vec<Duration>,
}

impl Samples {
    /// Collects another worker or measured run.
    pub fn append(&mut self, mut other: Self) {
        self.reads.append(&mut other.reads);
        self.writes.append(&mut other.writes);
    }
}

impl Bench {
    /// Warms and measures fresh states. Setup and the complete final-state
    /// check are outside the timer. Throughput includes worker startup;
    /// the operation latencies are supplied by the workload.
    pub fn measure_mixed<S>(
        &self,
        case: Case,
        mut setup: impl FnMut() -> S,
        mut run: impl FnMut(&S) -> Samples,
        mut check: impl FnMut(&S),
    ) {
        let key = (case.operation.clone(), case.threads, case.bytes);
        assert!(self.measured.borrow_mut().insert(key), "duplicate measurement");
        let mut rates = Vec::with_capacity(RUNS);
        let mut samples = Samples::default();
        for timed in [false].into_iter().chain([true; RUNS]) {
            let state = setup();
            let started = Instant::now();
            let found = run(&state);
            let secs = started.elapsed().as_secs_f64();
            check(&state);
            if timed {
                rates.push((found.reads.len() + found.writes.len()) as f64 / secs);
                samples.append(found);
            }
        }
        rates.sort_by(f64::total_cmp);
        self.mixed_latency(&case, "reads", &mut samples.reads);
        self.mixed_latency(&case, "writes", &mut samples.writes);
        self.report(case, Rates { median: rates[RUNS / 2], low: rates[0], high: rates[RUNS - 1] });
    }

    fn mixed_latency(&self, case: &Case, group: &str, samples: &mut [Duration]) {
        assert_ne!(samples, []);
        samples.sort_unstable();
        let percentile = |p: usize| samples[(samples.len() * p).div_ceil(100).saturating_sub(1)].as_secs_f64() * 1e6_f64;
        let (p50, p95, p99) = (percentile(50), percentile(95), percentile(99));
        if let Some(out) = self.out.borrow_mut().as_mut() {
            writeln!(
                out,
                "@latency\t{}\t{}, {group}\t{}\t{}\t{p50:.3}\t{p95:.3}\t{p99:.3}\tus",
                self.name,
                case.operation,
                case.threads,
                samples.len()
            )
            .unwrap();
        }
        note(&format!("{group}: {} samples; p50 {p50:.1} us, p95 {p95:.1} us, p99 {p99:.1} us (closed loop)", samples.len()));
    }
}
