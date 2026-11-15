//! \[DESIGN\] A paged R-link tree using R* subtree selection and splitting.
//! Readers follow split stamps (Kornacker and Banks, VLDB 1995), rather
//! than a B-link tree's ordered high key. Splits move entries only to a
//! new right sibling. Reinsertion and deletion condensation are excluded
//! because they move entries between already visited search branches.
//!
//! A full node is split before the insertion changes any entry. If page
//! allocation or parent posting fails, old entries remain reachable by
//! their right links, and the failed insertion has made no logical change.
//! A subsequent writer finishes the outstanding split. This matches the
//! index undo contract without adding physical log records: the database
//! checkpoints the pager's shadow and replays its logical row log (D25).
//!
//! Bounds are enlarged from the root down before a leaf gains an entry.
//! A writer rechecks each node's split stamp under its node lock: a stale
//! path retries instead of adding a point outside a newly split parent's
//! bounds. Counts of one row are serialized separately, so a changed
//! subtree choice cannot create a second copy of an existing entry.

mod nearest;
mod node;
mod scan;
mod write;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::bytes::{Put, Reader};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::locks::Locks;
use crate::error::{Code, Result, SqlError};
use crate::memory::MemoryBudget;
use crate::spatial::Bounds;
use crate::storage::RowId;
use crate::sync::lock_builtin;

pub use nearest::Nearest;
use node::{ENTRY_BYTES, HEADER, Node};
pub use scan::Scan;

/// The R-tree meta format: tag, format version, root page, root stamp.
const META_TAG: u8 = 82;
const META_VERSION: u8 = 1;
/// A fixed count of row-lock stripes, independent of tree size.
const ROW_LOCKS: u64 = 64;
/// At least four entries allow a split into two nonempty legal groups.
const MIN_CAPACITY: usize = 4;

/// A counted index of finite Cartesian rectangles and row identifiers.
/// Clones share the tree and its locks. A file must have one live tree
/// instance, with clones of that instance used for concurrent operations.
#[derive(Clone)]
pub struct RTree {
    inner: Arc<Inner>,
}

struct Inner {
    pool: Arc<BufferPool>,
    file: FileId,
    meta: PageId,
    capacity: usize,
    locks: Locks,
    rows: Vec<Mutex<()>>,
    splits: AtomicU64,
    right_moves: AtomicU64,
}

/// A remembered child and its split stamp. Only nodes split since this
/// stamp was read belong to its right-link chain.
#[derive(Clone, Copy)]
struct Visit {
    id: PageId,
    stamp: u64,
    level: u8,
}

impl RTree {
    /// Creates an empty tree in a registered pager file.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -910 if pages are too small.
    pub fn create(pool: Arc<BufferPool>, file: FileId) -> Result<RTree> {
        let capacity = capacity(&pool, file)?;
        let root = {
            let (root, mut page) = pool.new_page(file)?;
            *page = Node::empty(root, 0).bytes()?;
            root
        };
        let meta = {
            let (meta, mut page) = match pool.new_page(file) {
                Ok(made) => made,
                Err(error) => {
                    // An unpublished root has no readers. Failed cleanup
                    // leaks a page only; preserve the allocation error.
                    let _ = pool.free(file, root);
                    return Err(error);
                }
            };
            *page = meta_bytes(root, root.0);
            meta
        };
        Ok(Self::with(pool, file, meta, capacity))
    }

    /// Opens a tree at its meta page. Stored pages use the same format on
    /// all platforms; other versions are refused, never guessed.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 for malformed metadata.
    pub fn open(pool: Arc<BufferPool>, file: FileId, meta: PageId) -> Result<RTree> {
        let capacity = capacity(&pool, file)?;
        let tree = Self::with(pool, file, meta, capacity);
        tree.root()?;
        Ok(tree)
    }

    fn with(pool: Arc<BufferPool>, file: FileId, meta: PageId, capacity: usize) -> RTree {
        RTree {
            inner: Arc::new(Inner {
                pool,
                file,
                meta,
                capacity,
                locks: Locks::new(),
                rows: (0..ROW_LOCKS).map(|_| Mutex::new(())).collect(),
                splits: AtomicU64::new(0),
                right_moves: AtomicU64::new(0),
            }),
        }
    }

    /// The page needed to reopen the tree after a checkpoint.
    pub fn meta(&self) -> PageId {
        self.inner.meta
    }

    /// The number of structural splits and followed right links since
    /// this instance opened; observational counters, never search state.
    pub fn counts(&self) -> (u64, u64) {
        (self.inner.splits.load(Ordering::Relaxed), self.inner.right_moves.load(Ordering::Relaxed))
    }

    /// Adds one reference to `(bounds, row)`, returned once by a scan.
    ///
    /// # Errors
    ///
    /// Storage errors; SQLCODE -910 if its reference count would overflow.
    /// A failed call adds no reference. Structural splits may remain.
    pub fn insert(&self, bounds: Bounds, row: RowId) -> Result<()> {
        let stripe = usize::try_from(row % ROW_LOCKS).unwrap_or_default();
        let lock = self.inner.rows.get(stripe).ok_or_else(|| corrupt("a row-lock stripe is missing"))?;
        let _row = lock_builtin(lock);
        if self.change_existing(bounds, row, true)? {
            return Ok(());
        }
        self.add(bounds, row)
    }

    /// Removes one reference to `(bounds, row)`. An absent entry is a
    /// no-op. Empty nodes remain linked and can receive later inserts.
    ///
    /// # Errors
    ///
    /// Storage errors. A failed call removes no reference.
    pub fn remove(&self, bounds: Bounds, row: RowId) -> Result<()> {
        let stripe = usize::try_from(row % ROW_LOCKS).unwrap_or_default();
        let lock = self.inner.rows.get(stripe).ok_or_else(|| corrupt("a row-lock stripe is missing"))?;
        let _row = lock_builtin(lock);
        self.change_existing(bounds, row, false)?;
        Ok(())
    }

    /// Visits every entry intersecting `bounds`, or every entry if none.
    /// Entries held for the whole scan appear once even through splits;
    /// entries concurrently added or removed may or may not appear.
    pub fn scan(&self, bounds: Option<Bounds>) -> Scan {
        Scan::new(self.clone(), bounds)
    }

    /// Visits rectangles in increasing Euclidean distance from `query`,
    /// then row-id order. Taking the first k entries stops the traversal
    /// as soon as they are known. The frontier draws on `budget`.
    ///
    /// # Errors
    ///
    /// Storage errors; SQLCODE -918 if the frontier exceeds the budget.
    /// An error after construction is yielded once by the iterator.
    pub fn nearest(&self, query: Bounds, budget: &Arc<MemoryBudget>) -> Result<Nearest> {
        Nearest::new(self.clone(), query, budget)
    }

    /// Frees the nodes and meta page of a discarded tree. The caller must
    /// ensure no operation, including a cursor, will access it again.
    ///
    /// # Errors
    ///
    /// Storage errors or malformed links. Some pages may already be freed.
    pub fn free(&self) -> Result<()> {
        let mut walk = scan::Walk::new(self.clone(), None);
        while let Some((id, _)) = walk.node()? {
            self.inner.pool.free(self.inner.file, id)?;
        }
        self.inner.pool.free(self.inner.file, self.inner.meta)
    }

    fn root(&self) -> Result<Visit> {
        let page = self.inner.pool.read(self.inner.file, self.inner.meta.0)?;
        let mut r = Reader::new(&page, "R-tree metadata");
        let tag = r.u8()?;
        let version = r.u8()?;
        if tag != META_TAG || version != META_VERSION {
            return Err(r.corrupt("its tag or format version is unsupported"));
        }
        let id = PageId(r.u64()?);
        let stamp = r.u64()?;
        r.finish()?;
        drop(page);
        let level = self.read(id)?.level;
        Ok(Visit { id, stamp, level })
    }

    fn read(&self, id: PageId) -> Result<Node> {
        let page = self.inner.pool.read(self.inner.file, id.0)?;
        let node = Node::parse(&page)?;
        if node.right == Some(id) || node.entries.len() > self.inner.capacity {
            return Err(corrupt("a node links to itself or exceeds its capacity"));
        }
        Ok(node)
    }

    fn store(&self, id: PageId, node: &Node) -> Result<()> {
        let bytes = node.bytes()?;
        *self.inner.pool.write(self.inner.file, id)? = bytes;
        Ok(())
    }
}

fn capacity(pool: &BufferPool, file: FileId) -> Result<usize> {
    let capacity = pool.capacity(file)?.saturating_sub(HEADER).checked_div(ENTRY_BYTES).unwrap_or_default();
    if capacity < MIN_CAPACITY {
        return Err(SqlError::new(Code::Api, "[DESIGN] R-tree pages must hold at least four entries"));
    }
    Ok(capacity)
}

fn meta_bytes(root: PageId, stamp: u64) -> Vec<u8> {
    let mut bytes = vec![META_TAG, META_VERSION];
    bytes.put_u64(root.0);
    bytes.put_u64(stamp);
    bytes
}

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] an R-tree is malformed: {why}"))
}

#[cfg(test)]
mod tests;
