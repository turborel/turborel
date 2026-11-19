//! \[DESIGN\] How a transaction waits for a lock others hold: of a table
//! (`txn/locks.rs`), of a row (`txn/row_locks.rs`), or advisory
//! (`txn/advisory.rs`). Each wait goes through the one graph of who waits
//! for whom, so a cycle among waits of any kind is a deadlock found at
//! once (RULES.md D185, D187).

use std::fmt::Display;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use super::{LockWait, Stop, Transactions, TxId, Txn};
use crate::error::{Code, Result, SqlError};
use crate::sync::{Signal, lock, lock_builtin};

/// \[DESIGN\] How long a lock wait sleeps before it looks again for a
/// cancel of its statement (RULES.md D44), unless a release wakes it.
const STOP_POLL: Duration = Duration::from_millis(20);

/// The rule every refusal cites, by which the statement is undone and the
/// transaction goes on [X3.135-1986 §7.3 GR3b].
pub(super) const UNDONE: &str = "the statement is rejected and undone [X3.135-1986 §7.3 GR3b]";

/// What a wait does while the lock is held against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Waits {
    /// As the transaction's setting says (`LOCKS.WAIT`).
    Setting,
    /// \[EXTENSION\] Refused at once, of NOWAIT: lock not available.
    NoWait,
    /// \[EXTENSION\] Given up at once, of SKIP LOCKED, the caller told so.
    Skip,
}

/// \[DESIGN\] Who waits for a lock: a transaction, as its session and the
/// graph of waits know it.
#[derive(Clone)]
pub(crate) struct Waiter {
    pub(crate) id: TxId,
    pub(crate) stop: Arc<Stop>,
    pub(crate) lock_wait: LockWait,
    pub(crate) shared: Arc<Transactions>,
    pub(crate) running: Arc<Mutex<Option<ThreadId>>>,
}

impl Txn {
    /// Itself as a waiter for locks.
    pub(crate) fn waiter(&self) -> Waiter {
        Waiter {
            id: self.id,
            stop: Arc::clone(&self.stop),
            lock_wait: self.lock_wait,
            shared: Arc::clone(&self.shared),
            running: Arc::clone(&self.running),
        }
    }
}

/// The refusal of a lock request, giving `why`.
pub(super) fn refused(why: &str) -> SqlError {
    SqlError::new(Code::Conflict, format!("[DESIGN] {why}, so {UNDONE}"))
}

/// \[EXTENSION\] The refusal of a lock NOWAIT asked for, `what` being held.
pub(crate) fn not_available(what: &dyn Display) -> SqlError {
    SqlError::new(Code::Conflict, format!("[EXTENSION] {what} is locked by another transaction and NOWAIT was written, so {UNDONE}"))
        .with_state(crate::error::SqlState::LOCK_NOT_AVAILABLE)
}

/// The error for a wait whose lock a panicking thread poisoned.
pub(super) fn poisoned_wait() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a thread panicked while it held a lock (RULES.md D27)")
}

/// The holders `conflicts` finds of the state in `guarded`, each with
/// whether its running call is on this thread.
pub(super) type Conflicts<'c, T> = &'c dyn Fn(&T, ThreadId) -> Vec<(TxId, bool)>;

/// The state in `guarded` once `conflicts` finds no holder against `who`,
/// or the refusal of the wait, `what` named in it: Ok(false) if `waits` is
/// `Skip` and a holder is there. Its lock held still, `who` no longer among
/// those waiting.
///
/// # Errors
///
/// SQLCODE -910 if a lock is poisoned.
pub(super) fn wait_for<'l, T>(
    (who, waits): (&Waiter, Waits),
    what: &dyn Display,
    (guarded, released): (&'l Mutex<T>, &Signal),
    conflicts: Conflicts<'_, T>,
) -> Result<(Result<bool>, MutexGuard<'l, T>)> {
    let me = thread::current().id();
    let mut state = lock(guarded)?;
    let mut waited = false;
    let began = Instant::now();
    let outcome = loop {
        // A statement cancelled as it waits stops waiting (RULES.md D44).
        if let Err(e) = who.stop.check_now() {
            break Err(e);
        }
        let conflicting = conflicts(&state, me);
        if conflicting.is_empty() {
            break Ok(true);
        }
        if let Some(refusal) = at_once(who, waits, what, &conflicting) {
            break refusal;
        }
        let ids: Vec<TxId> = conflicting.iter().map(|(tx, _)| *tx).collect();
        let mut waits_for = lock(&who.shared.waits_for)?;
        if would_deadlock(&waits_for, who.id, &ids) {
            break Err(refused(&format!(
                "waiting for {what} would deadlock with another transaction (roll back to release this transaction's locks)"
            )));
        }
        if let Some(most) = who.stop.lock_timeout().filter(|&most| began.elapsed() >= most) {
            break Err(refused(&format!("{what} stayed locked by another transaction past LOCKS.TIMEOUT_MS, {} ms", most.as_millis())));
        }
        waits_for.insert(who.id, ids);
        drop(waits_for);
        waited = true;
        // Woken by a release, or now and then to look for a cancel or a
        // timeout.
        state = released.wait_timeout(state, STOP_POLL).map_err(|_| poisoned_wait())?.0;
    };
    if waited {
        lock(&who.shared.waits_for)?.remove(&who.id);
    }
    Ok((outcome, state))
}

/// What ends a wait at once, before it begins, if anything: SKIP LOCKED,
/// NOWAIT, a no-wait setting, or a holder running on this very thread.
fn at_once(who: &Waiter, waits: Waits, what: &dyn Display, conflicting: &[(TxId, bool)]) -> Option<Result<bool>> {
    match waits {
        Waits::Skip => return Some(Ok(false)),
        Waits::NoWait => return Some(Err(not_available(what))),
        Waits::Setting => {}
    }
    if who.lock_wait == LockWait::NoWait {
        return Some(Err(refused(&format!("{what} is locked by another transaction; locking is no-wait"))));
    }
    // A holder running a call on this very thread (a plug-in called back)
    // could never release the lock.
    if conflicting.iter().any(|(_, here)| *here) {
        return Some(Err(refused(&format!("{what} is locked by a transaction running on this thread, which could never release it"))));
    }
    None
}

/// Whether the call of a holder whose running thread is `running` is on
/// thread `me`.
pub(super) fn runs_here(running: &Mutex<Option<ThreadId>>, me: ThreadId) -> bool {
    *lock_builtin(running) == Some(me)
}

/// The transactions that wait for others: for each, those it waits for.
pub(super) type WaitsFor = std::collections::HashMap<TxId, Vec<TxId>>;

/// Whether `tx` waiting for `holders` would close a cycle of waiting
/// transactions back to `tx`: a deadlock. Searched without recursion, each
/// transaction once (RULES.md R-CF-6).
fn would_deadlock(waits_for: &WaitsFor, tx: TxId, holders: &[TxId]) -> bool {
    let mut seen: std::collections::HashSet<TxId> = std::collections::HashSet::new();
    let mut next: Vec<TxId> = holders.to_vec();
    while let Some(t) = next.pop() {
        if t == tx {
            return true;
        }
        if seen.insert(t) {
            next.extend(waits_for.get(&t).into_iter().flatten().copied());
        }
    }
    false
}
