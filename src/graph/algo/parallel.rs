//! \[DESIGN\] The algorithms' parallel runs: work cut into morsels, ranges
//! of vertices or of other items of a fixed size, that the run's threads
//! take one at a time from a shared count, so a thread whose morsels are
//! of few edges takes more of them and skewed degrees even out. Each
//! morsel's result is kept by its place, not by the thread that made it,
//! and a run's results are combined in morsel order, so they are the
//! same whatever the width, the same sums summed in the same order.
//!
//! The threads are scoped to the call that starts them
//! (`std::thread::scope`), named, and joined before it returns: they
//! live no longer than the statement's own call into the algorithm, so
//! they are not the thread manager's background work (RULES.md D55,
//! R-SAFE-5), which this file is listed beside as a place that starts
//! threads. The caller's own thread runs morsels too.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{Builder, scope};

use super::control::{Cancel, Control};
use crate::error::{Code, Result, SqlError};

/// The vertices of a morsel of per-vertex work: enough that taking one
/// costs little beside its work, few enough that a run of a few
/// thousand vertices still spreads over its threads.
pub(crate) const MORSEL: usize = 1024;

/// The prefix of a run's threads' names, each followed by its number.
const THREAD_NAME: &str = "graph-algo-";

/// The `k`th morsel of `0..n`, of `size` items.
fn span(k: usize, size: usize, n: usize) -> Range<usize> {
    let start = k.saturating_mul(size).min(n);
    start..start.saturating_add(size).min(n)
}

/// Runs `work` on each morsel of `0..n`, of `size` items, on up to the
/// control's width threads; the results in morsel order. The cancel is
/// checked before each morsel, and a morsel's error stops the run.
///
/// # Errors
///
/// A morsel's, the cancel's, or SQLCODE -918 if a thread cannot start
/// and -910 if one panicked.
pub(crate) fn morsels<T: Send, C: Cancel>(
    control: &Control<'_, C>,
    (n, size): (usize, usize),
    work: impl Fn(Range<usize>) -> Result<T> + Sync,
) -> Result<Vec<T>> {
    let size = size.max(1);
    let count = n.div_ceil(size);
    let threads = control.threads().min(count);
    if threads <= 1 {
        return (0..count).map(|k| control.check().and_then(|()| work(span(k, size, n)))).collect();
    }
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let run = || -> Result<Vec<(usize, T)>> {
        let mut done = Vec::new();
        loop {
            let k = next.fetch_add(1, Ordering::Relaxed);
            if k >= count || failed.load(Ordering::Acquire) {
                return Ok(done);
            }
            match control.check().and_then(|()| work(span(k, size, n))) {
                Ok(t) => done.push((k, t)),
                Err(e) => {
                    failed.store(true, Ordering::Release);
                    return Err(e);
                }
            }
        }
    };
    let mut all: Vec<(usize, T)> = Vec::with_capacity(count);
    for part in on_threads(threads, &run)? {
        all.extend(part?);
    }
    all.sort_unstable_by_key(|(k, _)| *k);
    Ok(all.into_iter().map(|(_, t)| t).collect())
}

/// Runs `work` on each morsel as `morsels` does, `fold` given each result
/// in morsel order as the run goes, at most a width's results held at
/// once: for results too big to keep one of each morsel, as the sums of
/// a morsel's sources are.
///
/// # Errors
///
/// As `morsels`, or `fold`'s.
pub(crate) fn folded<T: Send, C: Cancel>(
    control: &Control<'_, C>,
    (n, size): (usize, usize),
    work: impl Fn(Range<usize>) -> Result<T> + Sync,
    mut fold: impl FnMut(T) -> Result<()>,
) -> Result<()> {
    let size = size.max(1);
    let batch = control.threads().saturating_mul(size);
    let mut start = 0;
    while start < n {
        let end = start.saturating_add(batch).min(n);
        let offset = start;
        for t in
            morsels(control, (end.saturating_sub(start), size), |r| work(r.start.saturating_add(offset)..r.end.saturating_add(offset)))?
        {
            fold(t)?;
        }
        start = end;
    }
    Ok(())
}

/// The results of `run` on `threads` threads, the caller's own among
/// them, in thread order.
fn on_threads<R: Send>(threads: usize, run: &(impl Fn() -> R + Sync)) -> Result<Vec<R>> {
    scope(|s| {
        let mut handles = Vec::with_capacity(threads.saturating_sub(1));
        for k in 1..threads {
            let started = Builder::new().name(format!("{THREAD_NAME}{k}")).spawn_scoped(s, run);
            handles.push(
                started.map_err(|e| SqlError::new(Code::Resources, format!("[DESIGN] a graph algorithm's thread did not start: {e}")))?,
            );
        }
        let mut out = Vec::with_capacity(threads);
        out.push(run());
        for h in handles {
            out.push(h.join().map_err(|_| SqlError::new(Code::Api, "[DESIGN] a graph algorithm's thread panicked"))?);
        }
        Ok(out)
    })
}
