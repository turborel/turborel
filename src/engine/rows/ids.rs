//! \[DESIGN\] Row ids in blocks, one block per thread (RULES.md D31). Ids
//! given in one increasing sequence send every insert into a table to the
//! tree's rightmost leaf, whose lock the inserters then queue on, so
//! threads inserting at once go no faster than one. A thread instead takes
//! a block of `BLOCK` ids at a time and gives them in order, so threads
//! inserting at once fill different leaves. A session runs on one thread
//! at a time, so a thread stands for it.
//!
//! Ids stay unique and are never given twice; they increase within a
//! thread's block, not across threads. Ids a block leaves unused are never
//! given. A thread that has ended leaves its block behind; a shard of the
//! blocks holding `KEPT` forgets them all as another is taken, so threads
//! that come and go cost a bounded amount, and their blocks' unused ids
//! are skipped.
//! The meta page records the ids that may have been given, a reservation
//! moved past each block as it is taken, so ids go on past them when the
//! table is opened again.

use std::hash::BuildHasher;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::thread::{self, ThreadId};

use super::{KEY, RESERVE, RowTable, corrupt, meta_content};
use crate::error::{Code, Result, SqlError};
use crate::hash::murmur::{MurmurBuildHasher, MurmurMap};
use crate::storage::RowId;
use crate::sync::{lock, lock_builtin};

/// Ids a thread takes at a time: more than a leaf holds, so threads
/// inserting at once work in different leaves.
pub(super) const BLOCK: u64 = 4096;
/// Shards of the blocks, so threads taking ids rarely meet.
const SHARDS: u64 = 64;
/// The most blocks a shard keeps before forgetting them all.
const KEPT: usize = 64;

/// The ids left in a thread's block: `next` up to `end`, not included.
#[derive(Clone, Copy)]
struct Block {
    next: RowId,
    end: RowId,
}

/// \[DESIGN\] The blocks of row ids the threads inserting into a table hold.
pub(super) struct IdBlocks {
    shards: Vec<Mutex<MurmurMap<ThreadId, Block>>>,
}

impl IdBlocks {
    pub(super) fn new() -> IdBlocks {
        IdBlocks { shards: (0..SHARDS).map(|_| Mutex::new(MurmurMap::default())).collect() }
    }

    fn shard(&self, thread: ThreadId) -> Option<&Mutex<MurmurMap<ThreadId, Block>>> {
        usize::try_from(MurmurBuildHasher::default().hash_one(thread) % SHARDS).ok().and_then(|i| self.shards.get(i))
    }

    /// The calling thread's next id; when its block is used up, `take`
    /// gives the first id of a new block of `BLOCK`.
    pub(super) fn next(&self, take: impl FnOnce() -> Result<RowId>) -> Result<RowId> {
        let thread = thread::current().id();
        let Some(shard) = self.shard(thread) else { return take() };
        let mut blocks = lock_builtin(shard);
        let current = blocks.get(&thread).copied().filter(|b| b.next < b.end);
        let block = if let Some(block) = current {
            block
        } else {
            if blocks.len() >= KEPT {
                blocks.clear();
            }
            let start = take()?;
            Block { next: start, end: start.saturating_add(BLOCK) }
        };
        blocks.insert(thread, Block { next: block.next.saturating_add(1), ..block });
        Ok(block.next)
    }

    /// Forgets every block, so ids are next taken past those given so far:
    /// a row restored under an id may lie in a block.
    pub(super) fn clear(&self) {
        for shard in &self.shards {
            lock_builtin(shard).clear();
        }
    }
}

/// A row id as a key: big-endian, so keys sort as ids.
pub(super) fn key(id: RowId) -> [u8; KEY] {
    id.to_be_bytes()
}

/// The row id a key is.
pub(super) fn row_id(key: &[u8]) -> Result<RowId> {
    <[u8; KEY]>::try_from(key).map(RowId::from_be_bytes).map_err(|_| corrupt("a key is not a row id"))
}

/// The error of no row with id `id`.
pub(super) fn no_row<T>(id: RowId) -> Result<T> {
    Err(SqlError::new(Code::Api, format!("[DESIGN] no row with id {id}")))
}

impl RowTable {
    /// A new row id.
    pub(super) fn next_id(&self) -> Result<RowId> {
        self.blocks.next(|| {
            let start = self.next.fetch_add(BLOCK, Ordering::AcqRel);
            let last = start.checked_add(BLOCK - 1).filter(|&last| last < RowId::MAX);
            let last = last.ok_or_else(|| SqlError::new(Code::Unsupported, "[DESIGN] the table has given every row id"))?;
            self.reserve_past(last)?;
            Ok(start)
        })
    }

    /// Makes the meta page record `id` as possibly given.
    pub(super) fn reserve_past(&self, id: RowId) -> Result<()> {
        if id < self.reserved.load(Ordering::Acquire) {
            return Ok(());
        }
        let _reserving = lock(&self.reserving)?;
        if id < self.reserved.load(Ordering::Acquire) {
            return Ok(());
        }
        let reserved = id.saturating_add(RESERVE);
        *self.pool.write(self.file, self.meta)? = meta_content(self.tree.meta(), reserved, self.frozen.stored());
        self.reserved.store(reserved, Ordering::Release);
        Ok(())
    }

    /// Writes the meta page as the table is now.
    pub(super) fn write_meta(&self) -> Result<()> {
        let _reserving = lock(&self.reserving)?;
        *self.pool.write(self.file, self.meta)? =
            meta_content(self.tree.meta(), self.reserved.load(Ordering::Acquire), self.frozen.stored());
        Ok(())
    }
}
