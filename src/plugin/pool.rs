//! \[DESIGN\] The processes of one extension, and a call's round trip
//! (RULES.md D170, D172).
//!
//! A call leases a process: an idle one, else one started if fewer than
//! PLUGINS.PROCESSES run, else it waits for one. One found ended is let
//! go and replaced. A process that failed a call, by ending, breaking the
//! protocol, sending too much or not answering in time, is stopped and
//! never leased again, so the next call starts a fresh one; the call that
//! failed is not tried again, as its input may be what failed.
//!
//! While it waits for its answer, a call looks every `POLL_MS` at its
//! statement's cancellation (D44) and at its deadline. Past either, it
//! sends CANCEL and gives the program PLUGINS.CANCEL_GRACE_MS to answer;
//! a program that does not is stopped. A call of a procedure or a table
//! function may ask the host to run SQL as it runs (SQL, SQLDONE).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::error::{Result, SqlError};
use crate::memory::MemoryBudget;
use crate::sync::lock_builtin;
use crate::value::Value;

use super::errors::{raised, timed_out, too_long, unavailable, violation};
use super::limits::Limits;
use super::payload::statistics::Asked;
use super::process::{Next, Process, Spawn};
use super::wire::{Call, Message};

/// \[DESIGN\] How often, in milliseconds, a waiting call looks at its
/// statement's cancellation and its deadline: soon enough that a cancel
/// is felt at once, seldom enough to cost nothing.
pub const POLL_MS: u64 = 10;
/// The SQLSTATE an ERROR answering SQL a program asked for gives, if its
/// error has none: external routine exception.
const SQL_FAILED: &str = "38000";

/// SQL a program asks the host to run as a call runs: whether its rows
/// are wanted, its text and its arguments; its rows, none of a statement.
pub type SqlRunner<'a> = &'a mut dyn FnMut(bool, &str, &[Value]) -> Result<Vec<Vec<Value>>>;

/// What a call is told as it runs.
#[derive(Default)]
pub struct CallContext<'a> {
    /// Its statement's cancellation, if a statement runs it: an error once
    /// it is cancelled.
    pub interrupt: Option<&'a dyn Fn() -> Result<()>>,
    /// What runs SQL the program asks for, if it may.
    pub sql: Option<SqlRunner<'a>>,
}

/// A call: the kind and operation, the object, and its payload.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    /// The object's kind's code.
    pub kind: u8,
    /// The operation.
    pub op: u8,
    /// The object's name.
    pub object: &'a str,
    /// The payload (`payload.rs`).
    pub payload: Vec<u8>,
}

/// What a pool has done, for INFORMATION_SCHEMA.EXTENSIONS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolInfo {
    /// Processes running now.
    pub running: u64,
    /// Processes started.
    pub started: u64,
    /// Processes stopped after a failure.
    pub stopped: u64,
    /// Calls made.
    pub calls: u64,
    /// Calls that failed but by the program's own ERROR.
    pub failed: u64,
}

/// The processes of an extension.
pub struct Pool {
    spawn: Spawn,
    /// The executable, relative to the directory.
    executable: PathBuf,
    limits: Arc<Limits>,
    /// What checks its directory as each process starts.
    check: Option<super::host::Check>,
    /// What answers the statistics its programs ask (RULES.md D279).
    statistician: Option<super::host::Statistician>,
    /// Whether the host named its directory itself, as PLUGINS.HOST does,
    /// so PLUGINS.DIRECTORIES need not hold it (RULES.md D229).
    named: bool,
    /// The settings its program is told, if its manifest has any
    /// (RULES.md D282).
    feed: Option<Arc<super::settings::Feed>>,
    idle: Mutex<Idle>,
    freed: Condvar,
    next_call: AtomicU64,
    started: AtomicU64,
    stopped: AtomicU64,
    calls: AtomicU64,
    failed: AtomicU64,
}

/// The processes not leased, and how many run.
#[derive(Default)]
struct Idle {
    processes: Vec<Process>,
    running: usize,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool").field("extension", &self.spawn.extension).finish_non_exhaustive()
    }
}

impl Pool {
    /// The pool of the extension `extension`, in `directory`, its program
    /// `executable` there, serving the kinds `kinds`: no process started.
    pub fn new(
        extension: &str,
        (directory, executable): (PathBuf, PathBuf),
        kinds: Vec<u8>,
        host: (Arc<Limits>, Arc<MemoryBudget>),
    ) -> Pool {
        let (limits, memory) = host;
        let spawn = Spawn { extension: extension.to_owned(), program: PathBuf::new(), directory, kinds, memory };
        Pool {
            spawn,
            executable,
            limits,
            check: None,
            statistician: None,
            named: false,
            feed: None,
            idle: Mutex::new(Idle::default()),
            freed: Condvar::new(),
            next_call: AtomicU64::new(1),
            started: AtomicU64::new(0),
            stopped: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        }
    }

    /// It, `check` checking its directory as each process starts, as a
    /// package's digests are checked (RULES.md D173).
    #[must_use]
    pub fn with_check(mut self, check: super::host::Check) -> Pool {
        self.check = Some(check);
        self
    }

    /// It, `statistician` answering the statistics its programs ask
    /// (RULES.md D279).
    #[must_use]
    pub fn with_statistics(mut self, statistician: super::host::Statistician) -> Pool {
        self.statistician = Some(statistician);
        self
    }

    /// The statistics `asked` of a program as a call runs: a call that
    /// runs SQL, a procedure's or a table function's, by its user's
    /// privileges, checked by a query of what they are of that reads no
    /// row; another, of a kind that runs as the server, by the server's
    /// (RULES.md D173).
    fn statistics(&self, asked: &Asked, cx: &mut CallContext<'_>) -> Result<Vec<u8>> {
        let answer = self.statistician.as_ref().ok_or_else(|| violation("statistics are asked of a host that gives none"))?;
        let told = answer(asked)?;
        if let Some(run) = cx.sql.as_mut() {
            let quoted = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
            let read = told.column.as_deref().map_or_else(|| "1".to_owned(), quoted);
            let (schema, table) = &told.table;
            run(true, &format!("SELECT {read} FROM {}.{} WHERE 1 = 0", quoted(schema), quoted(table)), &[])?;
        }
        Ok(told.payload)
    }

    /// It, its directory named by the host itself, as a host plug-in's
    /// is, so PLUGINS.DIRECTORIES need not hold it (RULES.md D229).
    #[must_use]
    pub fn named_by_host(mut self) -> Pool {
        self.named = true;
        self
    }

    /// It, its program told the settings `feed` names before each call,
    /// as they change (RULES.md D282).
    #[must_use]
    pub fn with_feed(mut self, feed: Option<Arc<super::settings::Feed>>) -> Pool {
        self.feed = feed;
        self
    }

    /// The settings its program is told, if any.
    pub fn feed(&self) -> Option<&Arc<super::settings::Feed>> {
        self.feed.as_ref()
    }

    /// The extension's name.
    pub fn extension(&self) -> &str {
        &self.spawn.extension
    }

    /// What it has done.
    pub fn info(&self) -> PoolInfo {
        let running = u64::try_from(lock_builtin(&self.idle).running).unwrap_or(u64::MAX);
        let load = |n: &AtomicU64| n.load(Ordering::Relaxed);
        PoolInfo {
            running,
            started: load(&self.started),
            stopped: load(&self.stopped),
            calls: load(&self.calls),
            failed: load(&self.failed),
        }
    }

    /// Makes `request` of a process leased for it.
    ///
    /// # Errors
    ///
    /// The program's own error; or SQLSTATE 38T01-38T05, or the
    /// statement's cancellation (`Lease::call`).
    pub fn call(self: &Arc<Self>, request: &Request<'_>, cx: &mut CallContext<'_>) -> Result<Vec<u8>> {
        self.lease()?.call(request, cx)
    }

    /// A process of its own, kept until the lease is dropped: for an
    /// exchange or a handle a program keeps between calls.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T05 if none can be started; 38T02 if none is free within
    /// PLUGINS.CALL_TIMEOUT_MS.
    pub fn lease(self: &Arc<Self>) -> Result<Lease> {
        let deadline = self.limits.timeout().and_then(|t| Instant::now().checked_add(t));
        let mut idle = lock_builtin(&self.idle);
        loop {
            if let Some(mut process) = idle.processes.pop() {
                if !process.exited() {
                    return Ok(Lease { pool: Arc::clone(self), process: Some(process) });
                }
                // Ended as it waited: let go, stopped as it drops.
                idle.running = idle.running.saturating_sub(1);
                self.stopped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if idle.running < self.limits.processes() {
                idle.running = idle.running.saturating_add(1);
                drop(idle);
                return self.started();
            }
            let wait = deadline.map_or(Duration::MAX, |d| d.saturating_duration_since(Instant::now()));
            if wait.is_zero() {
                return Err(timed_out(self.limits.timeout_ms()));
            }
            idle = self.freed.wait_timeout(idle, wait).map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
        }
    }

    /// A lease of a process started now, its place counted already.
    fn started(self: &Arc<Self>) -> Result<Lease> {
        let begun = self.spawned().and_then(|spawn| Process::start(&spawn, &self.limits));
        match begun {
            Ok(process) => {
                self.started.fetch_add(1, Ordering::Relaxed);
                Ok(Lease { pool: Arc::clone(self), process: Some(process) })
            }
            Err(e) => {
                self.release(None);
                Err(e)
            }
        }
    }

    /// How to start its program now: its directory allowed by
    /// PLUGINS.DIRECTORIES as it is now (RULES.md D173), unless the host
    /// named it.
    fn spawned(&self) -> Result<Spawn> {
        let directory = if self.named { self.spawn.directory.clone() } else { self.limits.directories().allowed(&self.spawn.directory)? };
        if let Some(check) = &self.check {
            check(&directory)?;
        }
        let program = directory.join(&self.executable);
        if !program.starts_with(&directory) {
            return Err(unavailable("its executable is outside its directory"));
        }
        Ok(Spawn { program, directory, ..self.spawn.clone() })
    }

    /// `process` given back, if it may serve again; else its place freed.
    fn release(&self, process: Option<Process>) {
        let mut idle = lock_builtin(&self.idle);
        match process {
            Some(p) => idle.processes.push(p),
            None => idle.running = idle.running.saturating_sub(1),
        }
        drop(idle);
        self.freed.notify_one();
    }
}

/// Each idle process asked to end, and stopped.
impl Drop for Pool {
    fn drop(&mut self) {
        let processes = std::mem::take(&mut lock_builtin(&self.idle).processes);
        let grace = self.limits.grace();
        for p in processes {
            p.shut_down(grace);
        }
    }
}

/// A process leased from a pool: given back as it is dropped, unless a
/// call failed it.
pub struct Lease {
    pool: Arc<Pool>,
    process: Option<Process>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.pool.release(self.process.take());
    }
}

impl Lease {
    /// Makes `request`, the answer's payload.
    ///
    /// # Errors
    ///
    /// The program's ERROR, its SQLSTATE kept as `errors::raised` says;
    /// SQLSTATE 38T01 if the process ended or was stopped, 38T02 past
    /// PLUGINS.CALL_TIMEOUT_MS, 38T03 of a message out of place, 38T04
    /// past PLUGINS.MAX_CALL_BYTES; the statement's cancellation.
    pub fn call(&mut self, request: &Request<'_>, cx: &mut CallContext<'_>) -> Result<Vec<u8>> {
        let pool = Arc::clone(&self.pool);
        pool.calls.fetch_add(1, Ordering::Relaxed);
        let answer = self.round_trip(request, cx);
        if let Err(Failure::Runtime(_)) = &answer {
            pool.failed.fetch_add(1, Ordering::Relaxed);
        }
        answer.map_err(Failure::error)
    }

    /// The process, if it has not failed.
    fn process(&mut self) -> std::result::Result<&mut Process, Failure> {
        self.process.as_mut().ok_or_else(|| Failure::Runtime(super::errors::ended("an earlier call stopped it")))
    }

    /// The process stopped, after a failure `e` of it.
    fn stop(&mut self, e: SqlError) -> Failure {
        if self.process.take().is_some() {
            self.pool.stopped.fetch_add(1, Ordering::Relaxed);
        }
        Failure::Runtime(e)
    }

    /// `request` sent, and its answer waited for.
    fn round_trip(&mut self, request: &Request<'_>, cx: &mut CallContext<'_>) -> std::result::Result<Vec<u8>, Failure> {
        let call = self.pool.next_call.fetch_add(1, Ordering::Relaxed);
        let message = Message::Call(Call {
            call,
            kind: request.kind,
            op: request.op,
            object: request.object.to_owned(),
            payload: request.payload.clone(),
        });
        if let Some(feed) = self.pool.feed.clone() {
            let told = self.process()?.tell_settings(&feed);
            told.map_err(|e| self.stop(e))?;
        }
        let sent = self.process()?.send(&message);
        sent.map_err(|e| self.stop(e))?;
        let limits = Arc::clone(&self.pool.limits);
        let deadline = limits.timeout().and_then(|t| Instant::now().checked_add(t));
        let mut received = 0_u64;
        loop {
            if let Some(Err(e)) = cx.interrupt.map(|i| i()) {
                return Err(self.cancelled(call, e));
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(self.cancelled(call, timed_out(limits.timeout_ms())));
            }
            let next = self.process()?.next(Duration::from_millis(POLL_MS));
            let (got, bytes) = match next.map_err(|e| self.stop(e))? {
                Next::Waiting => continue,
                Next::Message(m, n) => (m, n),
            };
            received = received.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
            if received > limits.max_call() {
                return Err(self.stop(too_long(received, limits.max_call())));
            }
            if let Some(answer) = self.answered(call, (request.object, got), cx)? {
                return Ok(answer);
            }
        }
    }

    /// What `message`, received as call `call` of `object` runs, gives:
    /// its answer, if it is one.
    fn answered(
        &mut self,
        call: u64,
        (object, message): (&str, Message),
        cx: &mut CallContext<'_>,
    ) -> std::result::Result<Option<Vec<u8>>, Failure> {
        let of = message_call(&message);
        if of != Some(call) {
            return Err(self.stop(violation(&format!("a message of type {} answers no call running", message.code()))));
        }
        match message {
            Message::Result { payload, .. } => Ok(Some(payload)),
            Message::Error { state, message, .. } => Err(Failure::Own(raised(object, &state, &message))),
            Message::Sql { rows, text, args, .. } => {
                let done = match cx.sql.as_mut() {
                    Some(run) => run(rows, &text, &args),
                    None => Err(violation("SQL is asked of an object that runs none")),
                };
                let reply = match done {
                    Ok(made) => Message::SqlDone { call, rows: made },
                    Err(e) => Message::Error { call, state: sql_state(&e), message: e.msg },
                };
                let sent = self.process()?.send(&reply);
                sent.map_err(|e| self.stop(e))?;
                Ok(None)
            }
            Message::Statistics { asked, .. } => {
                let reply = match self.pool.statistics(&asked, cx) {
                    Ok(payload) => Message::StatisticsDone { call, payload },
                    Err(e) => Message::Error { call, state: sql_state(&e), message: e.msg },
                };
                let sent = self.process()?.send(&reply);
                sent.map_err(|e| self.stop(e))?;
                Ok(None)
            }
            Message::Hello(_)
            | Message::Welcome(_)
            | Message::Call(_)
            | Message::Cancel { .. }
            | Message::Shutdown
            | Message::Settings(_)
            | Message::SqlDone { .. }
            | Message::StatisticsDone { .. } => Err(self.stop(violation("a message a program never sends"))),
        }
    }

    /// Call `call` given up for `e`: CANCEL sent, and the program given
    /// PLUGINS.CANCEL_GRACE_MS to answer it, else stopped.
    fn cancelled(&mut self, call: u64, e: SqlError) -> Failure {
        let grace = self.pool.limits.grace();
        let until = Instant::now().checked_add(grace);
        let Ok(process) = self.process() else { return Failure::Runtime(e) };
        if process.send(&Message::Cancel { call }).is_err() {
            return self.stop(e);
        }
        while until.is_some_and(|u| Instant::now() < u) {
            let Ok(live) = self.process() else { break };
            match live.next(Duration::from_millis(POLL_MS)) {
                Ok(Next::Message(m, _)) => {
                    let answer = matches!(m, Message::Result { .. } | Message::Error { .. });
                    if answer && message_call(&m) == Some(call) {
                        return Failure::Runtime(e);
                    }
                }
                Ok(Next::Waiting) => {}
                Err(_) => break,
            }
        }
        self.stop(e)
    }
}

/// The call a message names, if it names one.
fn message_call(message: &Message) -> Option<u64> {
    match message {
        Message::Result { call, .. }
        | Message::Error { call, .. }
        | Message::Cancel { call }
        | Message::Sql { call, .. }
        | Message::SqlDone { call, .. }
        | Message::Statistics { call, .. }
        | Message::StatisticsDone { call, .. } => Some(*call),
        Message::Call(c) => Some(c.call),
        Message::Hello(_) | Message::Welcome(_) | Message::Shutdown | Message::Settings(_) => None,
    }
}

/// The SQLSTATE of `e`, as ERROR carries it.
fn sql_state(e: &SqlError) -> String {
    let state = e.sqlstate();
    if state.as_str().len() == SQL_FAILED.len() { state.as_str().to_owned() } else { SQL_FAILED.to_owned() }
}

/// How a call failed: by the program's own ERROR, its process kept; or
/// otherwise.
enum Failure {
    Own(SqlError),
    Runtime(SqlError),
}

impl Failure {
    /// Its error.
    fn error(self) -> SqlError {
        match self {
            Failure::Own(e) | Failure::Runtime(e) => e,
        }
    }
}

#[cfg(test)]
mod tests;
