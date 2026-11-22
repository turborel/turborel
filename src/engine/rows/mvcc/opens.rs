//! \[DESIGN\] The transactions open in a table, by id, in shards (RULES.md
//! D784). Each transaction's bookkeeping is found, begun and ended under
//! the lock of its shard alone, chosen by its id, each shard alone on its
//! cache lines with its count. Transactions running at once fall in
//! different shards, so one's begin, reads and end write no line
//! another's take; one lock for the table had every reader of it write
//! the lock's count of readers at each row. What asks of them all, the
//! newest or the oldest open and whether any is, looks at each shard in
//! turn, and reads only the counts to tell whether any is.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::Open;
use crate::hash::xxhash::Xxh3Map;
use crate::sync::Alone;
use crate::txn::TxId;

/// Shards of the map.
const SHARDS: u64 = 32;

/// The bookkeeping of the transactions of one shard.
type Map = Xxh3Map<TxId, Arc<Mutex<Open>>>;

/// One shard, and how many it holds, read without its lock.
#[derive(Default)]
struct Shard {
    map: RwLock<Map>,
    len: AtomicUsize,
}

/// The transactions open in a table.
pub(super) struct Opens {
    shards: Box<[Alone<Shard>]>,
}

impl Default for Opens {
    fn default() -> Opens {
        Opens { shards: (0..SHARDS).map(|_| Alone::default()).collect() }
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

    /// The bookkeeping of `tx`, if it is open here.
    pub(super) fn get(&self, tx: TxId) -> Option<Arc<Mutex<Open>>> {
        self.shard(tx).and_then(|shard| Self::read(shard).get(&tx).map(Arc::clone))
    }

    /// Opens `tx` here with what `made` makes, unless it is open.
    pub(super) fn begin(&self, tx: TxId, made: impl FnOnce() -> Open) {
        let Some(shard) = self.shard(tx) else { return };
        let mut map = Self::write(shard);
        map.entry(tx).or_insert_with(|| Arc::new(Mutex::new(made())));
        shard.len.store(map.len(), Ordering::SeqCst);
    }

    /// Ends `tx` here: its bookkeeping, if it was open.
    pub(super) fn remove(&self, tx: TxId) -> Option<Arc<Mutex<Open>>> {
        let shard = self.shard(tx)?;
        let mut map = Self::write(shard);
        let removed = map.remove(&tx);
        shard.len.store(map.len(), Ordering::SeqCst);
        removed
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
