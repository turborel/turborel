//! \[DESIGN\] The server's runtime (RULES.md D42, D55): the one place its
//! threads are made. Its threads are named `turborel-server-<n>`, so an
//! operator's tools tell them from the database's own
//! (`turborel-checkpoint`, `turborel-maintenance`). Its I/O threads are
//! tokio's default, one for each processor; its blocking pool, where
//! statements run, may grow to MAX_WORKERS, MOST_LOGINS for the checks of
//! logins, and a few spare, but is made only as they need it, and no more
//! than SERVER.WORKERS statements and SERVER.MAX_LOGINS checks run at once
//! (`server.rs`, `logins.rs`). `turboreld` and the tests' servers are made
//! by it alike. The thread writing the diagnostics' lines (`log.rs`) is
//! made here too, `turborel-server-log`, so a sink set before the runtime
//! is made, as the database opens, has its writer.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::runtime::{Builder, Runtime};

use crate::settings::{MAX_WORKERS, MOST_LOGINS};

/// \[DESIGN\] The blocking threads past MAX_WORKERS and MOST_LOGINS: for
/// the steps of a session that are neither statements nor a login's
/// checks, as its end, while every worker is busy. Fixed, not a setting
/// (RULES.md D36): a shape of the runtime, fixed as it is made.
const BLOCKING_SPARE: usize = 4;

/// The prefix of the names of the server's threads.
const LABEL: &str = "turborel-server";

/// \[DESIGN\] The thread that writes the diagnostics' lines (`log.rs`),
/// running `work`.
///
/// # Errors
///
/// The thread cannot be made.
pub(crate) fn writer(work: impl FnOnce() + Send + 'static) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name(format!("{LABEL}-log")).spawn(work)
}

/// The server's runtime: I/O threads as many as `io_threads` says, else
/// one for each processor.
///
/// # Errors
///
/// The runtime cannot be made.
pub fn runtime(io_threads: Option<usize>) -> std::io::Result<Runtime> {
    let made = Arc::new(AtomicUsize::new(0));
    let mut builder = Builder::new_multi_thread();
    builder
        .enable_all()
        .max_blocking_threads(MAX_WORKERS.saturating_add(MOST_LOGINS).saturating_add(BLOCKING_SPARE))
        .thread_name_fn(move || format!("{LABEL}-{}", made.fetch_add(1, Ordering::Relaxed)));
    if let Some(n) = io_threads {
        builder.worker_threads(n);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D55: the server's threads, its I/O
    /// threads and its blocking pool's, are named as the server's.
    #[test]
    fn its_threads_are_named() {
        let runtime = runtime(Some(1)).unwrap();
        let named = || std::thread::current().name().map(str::to_owned);
        let io = runtime.block_on(runtime.spawn(async move { named() })).unwrap();
        let blocking = runtime.block_on(runtime.spawn_blocking(named)).unwrap();
        for name in [io, blocking] {
            assert!(name.as_deref().is_some_and(|n| n.starts_with("turborel-server-")), "{name:?}");
        }
    }
}
