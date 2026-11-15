//! \[DESIGN\] Node locks shared by the paged tree implementations:
//! a lock keeps other writers off a node, never readers, who read a node
//! under the buffer pool's latch for the moment it takes. Each tree
//! defines the order in which writers acquire these locks. Held locks
//! are kept by shard, so writers of different nodes rarely meet.

use std::sync::{Mutex, PoisonError};

use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::sync::{Signal, lock_builtin};

/// Shards of the held locks.
const SHARDS: u64 = 64;

#[derive(Default)]
struct Shard {
    /// The pages locked in this shard: few at a time, as each writer holds
    /// only a few at a time.
    held: Mutex<Vec<PageId>>,
    released: Signal,
}

/// The locks of one tree's nodes.
pub(crate) struct Locks {
    shards: Vec<Shard>,
}

/// A held lock, released when dropped.
pub(crate) struct NodeLock<'a> {
    shard: &'a Shard,
    /// The node locked.
    pub(crate) id: PageId,
}

impl Locks {
    pub(crate) fn new() -> Locks {
        Locks { shards: (0..SHARDS).map(|_| Shard::default()).collect() }
    }

    /// Locks node `id`, waiting while another writer holds it. Locks are
    /// held over no plug-in call, so a poisoned shard is used as it is.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the shard of `id` is missing, which `new` prevents.
    pub(crate) fn lock(&self, id: PageId) -> Result<NodeLock<'_>> {
        let shard = usize::try_from(id.0 % SHARDS)
            .ok()
            .and_then(|i| self.shards.get(i))
            .ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a shard of the tree's locks is missing"))?;
        let mut held =
            shard.released.wait_while(lock_builtin(&shard.held), |held| held.contains(&id)).unwrap_or_else(PoisonError::into_inner);
        held.push(id);
        Ok(NodeLock { shard, id })
    }
}

impl Drop for NodeLock<'_> {
    fn drop(&mut self) {
        let mut held = lock_builtin(&self.shard.held);
        held.retain(|&h| h != self.id);
        drop(held);
        self.shard.released.notify_all();
    }
}
