//! How a case is measured: its terminals, each on a thread of its own and
//! a session of its own, run for the set seconds, once untimed and then
//! five times, on the harness, which reports the median transactions per
//! second. Beside it go the figures the timed runs make together.
//!
//! The time per transaction is portable only as wall time: the standard
//! library reads no processor time. It is reported as the time of the
//! cores the terminals could keep busy, the wall time times the lesser of
//! the threads and the processor's parallelism, per transaction. With
//! commits that sync, that includes the waits for the disk, which
//! `TURBOREL_OLTP_SYNC=off` takes out; then, at no more threads than
//! cores, it is the processor time a transaction costs, a bound above it
//! otherwise. Allocations are not counted: that needs a global allocator
//! of the bench's own, which is unsafe code.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::client::Client;
use crate::harness::note;

/// The figures of a case's timed runs, together.
#[derive(Default)]
pub struct Totals {
    /// The runs so far, the untimed one included.
    pub runs: usize,
    /// The timed runs' wall time.
    pub wall: Duration,
    /// Their transactions.
    pub transactions: u64,
}

impl Totals {
    /// Counts a run of `transactions` in `wall`; whether it was timed.
    pub fn count(&mut self, wall: Duration, transactions: u64) -> bool {
        self.runs += 1;
        if self.runs == 1 {
            return false;
        }
        self.wall += wall;
        self.transactions += transactions;
        true
    }

    /// The busy cores' microseconds per transaction at `threads`.
    pub fn micros_per_transaction(&self, threads: usize) -> f64 {
        let cores = cores();
        self.wall.as_secs_f64() * threads.min(cores) as f64 * 1e6_f64 / self.transactions.max(1) as f64
    }

    /// Prints the transactions' cost.
    pub fn report(&self, threads: usize) {
        let cores = cores();
        note(&format!(
            "{:.1} us a transaction of {} busy cores (wall x min(threads, {cores} cores)), {} transactions in {:.1} s",
            self.micros_per_transaction(threads),
            threads.min(cores),
            self.transactions,
            self.wall.as_secs_f64()
        ));
    }
}

/// The variable that sets the cores the terminals may keep busy, where the
/// processor's parallelism is not what the process may use.
const CORES_VAR: &str = "TURBOREL_OLTP_CORES";

/// The cores the terminals may keep busy: `CORES_VAR`, or the processor's
/// parallelism as the process sees it.
pub fn cores() -> usize {
    std::env::var(CORES_VAR).ok().and_then(|c| c.parse().ok()).unwrap_or_else(|| thread::available_parallelism().map_or(1, usize::from))
}

/// Runs `terminal(t, until)` for `t` in `0..threads`, each on a thread of
/// its own, until `secs` from now: what each gave, and the wall time.
pub fn on_threads<T: Send>(threads: usize, secs: Duration, terminal: impl Fn(usize, Instant) -> T + Sync) -> (Vec<T>, Duration) {
    let results = Mutex::new(Vec::with_capacity(threads));
    let started = Instant::now();
    let until = started + secs;
    thread::scope(|s| {
        for t in 0..threads {
            let (terminal, results) = (&terminal, &results);
            s.spawn(move || {
                let done = terminal(t, until);
                results.lock().unwrap().push(done);
            });
        }
    });
    (results.into_inner().unwrap(), started.elapsed())
}

/// The calls of each statement, the rows they gave or wrote, and the rows
/// they read, as the database's statement statistics count them.
pub type Statements = BTreeMap<String, [f64; 3]>;

/// The statements whose text is `like`, in upper case.
pub fn statements(client: &mut impl Client, like: &str) -> Statements {
    let sql = format!(
        "SELECT STATEMENT_TEXT, CALLS, ROW_COUNT, ROWS_READ FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS \
            WHERE UPPER(STATEMENT_TEXT) LIKE '{like}'"
    );
    let Ok(found) = client.direct(&sql, true) else { return Statements::new() };
    let mut all = Statements::new();
    for row in found {
        let number = |i: usize| row[i].as_f64().unwrap_or(0.0_f64);
        let text = if let turborel::value::Value::Char(s) = &row[0] { s.to_string() } else { String::new() };
        let counts = all.entry(text).or_default();
        for (i, count) in counts.iter_mut().enumerate() {
            *count += number(i + 1);
        }
    }
    all
}

/// Prints each statement's calls between `before` and `after`, and the rows
/// a call gave or wrote and read, under `heading`.
pub fn statement_rows(heading: &str, before: &Statements, after: &Statements) {
    note(heading);
    for (text, now) in after {
        let then = before.get(text).copied().unwrap_or_default();
        let calls = now[0] - then[0];
        if calls < 1.0_f64 {
            continue;
        }
        let (rows, read) = ((now[1] - then[1]) / calls, (now[2] - then[2]) / calls);
        note(&format!("  {text}: {calls:.0} calls, {rows:.2} rows a call, {read:.2} read"));
    }
}

/// The searches made of each index of the schema and the entries they read.
pub fn index_use(client: &mut impl Client) -> Statements {
    let sql = "SELECT INDEX_NAME, SEARCHES, ENTRIES_READ, ENTRY_COUNT FROM INFORMATION_SCHEMA.INDEX_STATISTICS";
    let Ok(found) = client.direct(sql, true) else { return Statements::new() };
    found
        .into_iter()
        .map(|row| {
            let number = |i: usize| row[i].as_f64().unwrap_or(0.0_f64);
            let name = if let turborel::value::Value::Char(s) = &row[0] { s.to_string() } else { String::new() };
            (name, [number(1), number(2), number(3)])
        })
        .collect()
}

/// Prints each index's searches between `before` and `after` and the
/// entries a search read, with its entries at `after`; the index statistics
/// count no dead entries passed apart from live ones.
pub fn index_rows(before: &Statements, after: &Statements, transactions: u64) {
    note("index searches in the timed runs:");
    for (name, now) in after {
        let then = before.get(name).copied().unwrap_or_default();
        let searches = now[0] - then[0];
        if searches < 1.0_f64 {
            continue;
        }
        let read = (now[1] - then[1]) / searches;
        let each = searches / transactions.max(1) as f64;
        note(&format!("  {name}: {each:.2} searches a transaction, {read:.2} entries read a search, {:.0} entries", now[2]));
    }
}
