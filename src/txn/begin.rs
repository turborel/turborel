//! \[DESIGN\] How a transaction begins (RULES.md D840), at no cost shared
//! with transactions on other threads but its id and its snapshot. What
//! every transaction of a session refers to, the coordinator, the log and
//! where its undo entries spill, is held in one `Origin` its session keeps
//! while its snapshots give the same, so a transaction takes one reference
//! to the session's own, not one to each object every session shares,
//! whose counts would move between every core at every begin and end. Its
//! id is given on its thread's stripe (`clock.rs`), so the registries by
//! id it enters, its snapshot's stripe, its end (`row_waits.rs`) and its
//! stop (`stop.rs`), are its thread's shards, written on its own core.

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, OnceLock};

use super::undo::{UndoLog, UndoSpace};
use super::{Beginning, Transactions, TxState, Txn};
use crate::log::Log;

/// What a transaction begins from: the coordinator, the log its changes and
/// its end are written to, if any, and where its undo entries spill.
pub(crate) struct Origin {
    pub(super) shared: Arc<Transactions>,
    pub(super) log: Option<Arc<dyn Log>>,
    space: Arc<UndoSpace>,
}

impl Origin {
    /// The origin of `shared`'s transactions, logging to `log`, spilling in
    /// `space`.
    pub(crate) fn new(shared: &Arc<Transactions>, log: Option<&Arc<dyn Log>>, space: UndoSpace) -> Origin {
        Origin { shared: Arc::clone(shared), log: log.map(Arc::clone), space: Arc::new(space) }
    }

    /// Whether it is of `shared`'s transactions, logging to `log`: so a
    /// session keeps it across the snapshots it takes while none of them
    /// changes these or its space (`State::origin`).
    pub(crate) fn is_of(&self, shared: &Arc<Transactions>, log: Option<&Arc<dyn Log>>) -> bool {
        let same_log = match (&self.log, log) {
            (Some(mine), Some(theirs)) => Arc::ptr_eq(mine, theirs),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        };
        Arc::ptr_eq(&self.shared, shared) && same_log
    }

    /// Where its transactions' undo entries spill.
    pub(crate) fn space(&self) -> &UndoSpace {
        &self.space
    }

    /// [X3.135-1986 §7.3 GR2a] a transaction is effectively initiated. Its
    /// id is new, later than every one before it; 2^58 ids cannot run out
    /// in practice. Its snapshot is the durable timestamp then. Its changes
    /// and its end go to the log, if there is one (RULES.md D25). Its undo
    /// entries spill in the origin's space.
    pub(crate) fn begin(self: &Arc<Self>, (lock_wait, reading, stop): Beginning) -> Txn {
        let shared = &self.shared;
        let (id, snapshot) = shared.clock.begin();
        shared.stops.insert(id, Arc::clone(&stop));
        shared.ends.begin(id);
        Txn {
            id,
            snapshot: AtomicU64::new(snapshot),
            reading,
            checking: AtomicBool::new(false),
            lock_wait,
            sync_commit: AtomicBool::new(true),
            open: AtomicBool::new(true),
            origin: Arc::clone(self),
            state: Mutex::new(TxState { undo: UndoLog::new(Arc::clone(&self.space)), ..TxState::default() }),
            running: Arc::new(Mutex::new(None)),
            stop,
            stamp: OnceLock::new(),
            pending: OnceLock::new(),
            depends: AtomicU64::new(0),
        }
    }
}

#[cfg(test)]
impl Transactions {
    /// A transaction of these, as a session's would begin, from an origin
    /// of its own: for tests.
    pub(super) fn begin(self: &Arc<Self>, log: Option<&Arc<dyn Log>>, beginning: Beginning, space: UndoSpace) -> Txn {
        Arc::new(Origin::new(self, log, space)).begin(beginning)
    }
}
