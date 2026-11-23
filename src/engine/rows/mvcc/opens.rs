//! \[DESIGN\] The transactions open in a table, by id, in shards (RULES.md
//! D784). Each transaction's bookkeeping is found, begun and ended under
//! the lock of its shard alone, chosen by its id, each shard alone on its
//! cache lines with its count. Transactions running at once fall in
//! different shards, so one's begin, reads and end write no line
//! another's take; one lock for the table had every reader of it write
//! the lock's count of readers at each row. What asks of them all, the
//! newest or the oldest open and whether any is, looks at each shard in
//! turn, and reads only the counts to tell whether any is.

use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::Open;
use crate::hash::xxhash::Xxh3Map;
use crate::sync::Alone;
use crate::txn::TxId;

/// Shards of the map.
const SHARDS: u64 = 32;

/// \[DESIGN\] A transaction's bookkeeping in a table, under its lock, and
/// whether the transaction has ended there, read without it (RULES.md
/// D871). What still holds the bookkeeping once the transaction has ended,
/// a scan's `Near` or a thread (`found.rs`), finds so before it uses it.
pub(super) struct Opened {
    ended: AtomicBool,
    open: Mutex<Open>,
}

impl Opened {
    /// Whether its transaction has ended in the table.
    pub(super) fn ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }
}

impl Deref for Opened {
    type Target = Mutex<Open>;

    fn deref(&self) -> &Mutex<Open> {
        &self.open
    }
}

/// The bookkeeping of the transactions of one shard.
pub(super) type Map = Xxh3Map<TxId, Arc<Opened>>;

/// One shard, and how many it holds, read without its lock.
#[derive(Default)]
struct Shard {
    map: RwLock<Map>,
    len: AtomicUsize,
}

/// The transactions open in a table.
pub(super) struct Opens {
    shards: Box<[Alone<Shard>]>,
    /// What names the table to a thread keeping bookkeeping of it
    /// (`found.rs`), an allocation of the table's own.
    token: Arc<()>,
}

impl Default for Opens {
    fn default() -> Opens {
        Opens { shards: (0..SHARDS).map(|_| Alone::default()).collect(), token: Arc::new(()) }
    }
}

impl Opens {
    fn shard(&self, tx: TxId) -> Option<&Shard> {
        usize::try_from(tx % SHARDS).ok().and_then(|at| self.shards.get(at)).map(|shard| &shard.0)
    }

    fn read(shard: &Shard) -> RwLockReadGuard<'_, Map> {
        shard.map.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(shard: &Shard) -> RwLockWriteGuard<'_, Map> {
        shard.map.write().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What names the table (`found.rs`).
    pub(super) fn token(&self) -> &Arc<()> {
        &self.token
    }

    /// The bookkeeping of `tx`, if it is open here.
    pub(super) fn get(&self, tx: TxId) -> Option<Arc<Opened>> {
        self.shard(tx).and_then(|shard| Self::read(shard).get(&tx).map(Arc::clone))
    }

    /// Opens `tx` here with what `made` makes, unless it is open.
    pub(super) fn begin(&self, tx: TxId, made: impl FnOnce() -> Open) {
        let Some(shard) = self.shard(tx) else { return };
        let mut map = Self::write(shard);
        map.entry(tx).or_insert_with(|| Arc::new(Opened { ended: AtomicBool::new(false), open: Mutex::new(made()) }));
        shard.len.store(map.len(), Ordering::SeqCst);
    }

    /// Ends `tx` here: its bookkeeping, if it was open, marked ended.
    pub(super) fn remove(&self, tx: TxId) -> Option<Arc<Opened>> {
        let shard = self.shard(tx)?;
        let mut map = Self::write(shard);
        let removed = map.remove(&tx);
        if let Some(opened) = &removed {
            opened.ended.store(true, Ordering::Release);
        }
        shard.len.store(map.len(), Ordering::SeqCst);
        removed
    }

    /// Every shard's map, held for writing: for tests that a call takes
    /// none.
    #[cfg(test)]
    pub(super) fn held(&self) -> Vec<RwLockWriteGuard<'_, Map>> {
        self.shards.iter().map(|shard| Self::write(shard)).collect()
    }

    /// Whether no transaction is open here.
    pub(super) fn is_empty(&self) -> bool {
        self.shards.iter().all(|shard| shard.len.load(Ordering::SeqCst) == 0)
    }

    /// The newest transaction open here but `tx`.
    pub(super) fn newest_but(&self, tx: TxId) -> Option<TxId> {
        self.shards.iter().filter_map(|shard| Self::read(shard).keys().filter(|&&by| by != tx).max().copied()).max()
    }

    /// The oldest transaction open here.
    pub(super) fn oldest(&self) -> Option<TxId> {
        self.shards.iter().filter_map(|shard| Self::read(shard).keys().min().copied()).min()
    }
}

#[cfg(test)]
mod tests;
