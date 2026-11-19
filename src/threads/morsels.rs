//! \[DESIGN\] Scoped workers taking morsels of work (RULES.md R-SAFE-5),
//! as parallel path search runs them [GS §17]: as many as the caller's
//! width (EXECUTOR.GRAPH_THREADS), each worker its own run of them, stealing the last morsels of
//! another's once its own are done, so skewed degrees even out.
//!
//! The workers are threads of the statement that runs them, joined
//! before it goes on, each named `turborel-graph-<n>` (RULES.md D55). A
//! failure in one stops the others taking morsels; each looks at the
//! statement's cancellation as it takes one (RULES.md D44). Results are
//! put back in the order of the morsels, so a parallel search gives what
//! a sequential one does.

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;
use crate::txn::Stop;

/// The name of a worker's thread, before its number.
const WORKER_NAME: &str = "turborel-graph";

/// The morsels each worker begins with, by worker.
fn shares(width: usize, morsels: usize) -> Vec<Mutex<VecDeque<usize>>> {
    let per = morsels.div_ceil(width.max(1));
    (0..width).map(|w| Mutex::new((w.saturating_mul(per)..w.saturating_add(1).saturating_mul(per).min(morsels)).collect())).collect()
}

/// The next morsel of worker `me`: its own first, else the last of
/// another's.
fn take(queues: &[Mutex<VecDeque<usize>>], me: usize) -> Option<usize> {
    if let Some(own) = queues.get(me).and_then(|q| lock_builtin(q).pop_front()) {
        return Some(own);
    }
    let width = queues.len();
    (1..width).find_map(|offset| {
        me.saturating_add(offset).checked_rem(width).and_then(|other| queues.get(other)).and_then(|q| lock_builtin(q).pop_back())
    })
}

/// Worker `me`'s loop: each morsel it takes, until none is left or one
/// has failed, its result paired with the morsel's number.
fn worker<T>(
    me: usize,
    (queues, failed): (&[Mutex<VecDeque<usize>>], &AtomicBool),
    stop: Option<&Arc<Stop>>,
    work: impl Fn(usize) -> Result<T>,
) -> Result<Vec<(usize, T)>> {
    let mut mine = Vec::new();
    while let Some(m) = take(queues, me).filter(|_| !failed.load(Ordering::Acquire)) {
        let result = stop.map_or(Ok(()), |s| s.check()).and_then(|()| work(m));
        if result.is_err() {
            failed.store(true, Ordering::Release);
        }
        mine.push((m, result?));
    }
    Ok(mine)
}

/// Runs `work` on each morsel of `size` items of `0..items`, on `width`
/// workers: the results, in the order of the morsels.
///
/// # Errors
///
/// The first error of a morsel, SQLCODE -920 if `stop` cancels, or -910
/// if a worker could not be started or failed.
pub(crate) fn morsels<T, F>(width: usize, (items, size): (usize, usize), stop: Option<&Arc<Stop>>, work: F) -> Result<Vec<T>>
where
    T: Send,
    F: Fn(Range<usize>) -> Result<T> + Sync,
{
    let size = size.max(1);
    let count = items.div_ceil(size);
    let range = |m: usize| m.saturating_mul(size)..m.saturating_add(1).saturating_mul(size).min(items);
    if width <= 1 || count <= 1 {
        return (0..count).map(|m| work(range(m))).collect();
    }
    let queues = shares(width, count);
    let failed = AtomicBool::new(false);
    let mut done: Vec<(usize, T)> = thread::scope(|scope| -> Result<Vec<(usize, T)>> {
        let mut handles = Vec::new();
        for me in 0..width {
            let (queues, failed, work, range) = (&queues, &failed, &work, &range);
            let run = move || worker(me, (queues, failed), stop, |m| work(range(m)));
            let handle = thread::Builder::new()
                .name(format!("{WORKER_NAME}-{me}"))
                .spawn_scoped(scope, run)
                .map_err(|e| SqlError::new(Code::Api, format!("[DESIGN] a graph search worker could not be started: {e}")))?;
            handles.push(handle);
        }
        let mut all = Vec::new();
        for handle in handles {
            let part = handle.join().map_err(|_| SqlError::new(Code::Api, "[DESIGN] a graph search worker failed"))?;
            all.extend(part?);
        }
        Ok(all)
    })?;
    done.sort_unstable_by_key(|d| d.0);
    Ok(done.into_iter().map(|d| d.1).collect())
}
