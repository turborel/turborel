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
//!
//! An index built whole is packed bottom-up (`pack.rs`), and leaves that
//! deletes empty or leave under minimum fill are taken out or merged into
//! a new node, their pages freed once no operation can still reach them
//! (`reclaim.rs`, RULES.md D139). Freed pages are allocated again, so split
//! stamps come from a counter in the meta page, never from page numbers.

mod estimate;
mod join;
mod nearest;
mod node;
mod pack;
mod reclaim;
mod scan;
mod shape;
mod write;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::bytes::{Put, Reader};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::locks::Locks;
use crate::engine::tree::{Entered, Epochs};
use crate::error::{Code, Result, SqlError};
use crate::memory::MemoryBudget;
use crate::spatial::Bounds;
use crate::storage::RowId;
use crate::sync::lock_builtin;

pub use join::{Join, Joined};
pub use nearest::Nearest;
use node::{ENTRY_BYTES, HEADER, Node};
pub use scan::Scan;
pub use shape::Level;

/// The R-tree meta format: tag, format version, root page, root stamp and
/// the next split stamp to give.
const META_TAG: u8 = 82;
const META_VERSION: u8 = 1;
/// A fixed count of row-lock stripes, independent of tree size.
const ROW_LOCKS: u64 = 64;
/// At least four entries allow a split into two nonempty legal groups.
const MIN_CAPACITY: usize = 4;
/// The stamp of a new tree's root; the counter gives the ones after it.
const FIRST_STAMP: u64 = 1;

/// A counted index of finite Cartesian rectangles and row identifiers.
/// Clones share the tree and its locks. Each tree locator must have one
/// live instance, with its clones used for concurrent operations.
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
    /// The operations under way by epoch, and the pages taken out of the
    /// tree to free once none can hold them (`reclaim.rs`).
    epochs: Epochs,
    /// Pages retired and not yet freed, so a remove looks for frees only
    /// when there are some.
    retired: AtomicU64,
    reclaimed: AtomicU64,
    narrowed: AtomicU64,
}

/// The meta page's fields.
#[derive(Clone, Copy)]
struct Meta {
    root: PageId,
    stamp: u64,
    next: u64,
}

/// An operation or scan counted in the tree's epoch until it is dropped,
/// so no page it may still read is freed (RULES.md D139).
pub(super) struct Pin {
    inner: Arc<Inner>,
    entered: Entered,
}

impl Drop for Pin {
    fn drop(&mut self) {
        self.inner.epochs.leave(self.entered);
    }
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
        Self::create_from(pool, file, FIRST_STAMP)
    }

    /// A new tree whose stamps start at `first`.
    fn create_from(pool: Arc<BufferPool>, file: FileId, first: u64) -> Result<RTree> {
        let capacity = capacity(&pool, file)?;
        let root = {
            let (root, mut page) = pool.new_page(file)?;
            *page = Node::empty(first, 0).bytes()?;
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
            *page = meta_bytes(Meta { root, stamp: first, next: first.saturating_add(1) });
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
                epochs: Epochs::new(),
                retired: AtomicU64::new(0),
                reclaimed: AtomicU64::new(0),
                narrowed: AtomicU64::new(0),
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

    /// The nodes taken out or merged, and the ancestor entries narrowed,
    /// since this instance opened; observational counters, never search
    /// state.
    pub fn reclaimed(&self) -> (u64, u64) {
        (self.inner.reclaimed.load(Ordering::Relaxed), self.inner.narrowed.load(Ordering::Relaxed))
    }

    /// Counts an operation in the tree's epoch while the pin lives.
    pub(super) fn pin(&self) -> Pin {
        Pin { inner: Arc::clone(&self.inner), entered: self.inner.epochs.enter() }
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
        let _pin = self.pin();
        self.insert_counted(bounds, row, 1)
    }

    fn insert_counted(&self, bounds: Bounds, row: RowId, count: u64) -> Result<()> {
        if self.change_existing(bounds, row, write::Delta::Add(count))?.is_some() {
            return Ok(());
        }
        self.add(bounds, row, count)
    }

    /// Removes one reference to `(bounds, row)`. An absent entry is a
    /// no-op. A leaf left empty or under minimum fill is taken out or
    /// merged with a sibling, its page freed once no scan can reach it.
    ///
    /// # Errors
    ///
    /// Storage errors. A failed call removes no reference.
    pub fn remove(&self, bounds: Bounds, row: RowId) -> Result<()> {
        let stripe = usize::try_from(row % ROW_LOCKS).unwrap_or_default();
        let lock = self.inner.rows.get(stripe).ok_or_else(|| corrupt("a row-lock stripe is missing"))?;
        let _row = lock_builtin(lock);
        let pin = self.pin();
        if let Some(leaf) = self.change_existing(bounds, row, write::Delta::Remove)? {
            // Reclamation only gives space back. The reference is already
            // removed, so its failure leaves a sparse leaf, never a wrong one.
            let _ = self.reclaim(leaf, bounds);
        }
        drop(pin);
        let _ = self.release();
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
        let _pin = self.pin();
        Nearest::new(self.clone(), query, budget)
    }

    /// Candidates ordered by monotone envelope lower bounds. Decline if
    /// the query or root envelope exceeds the caller's finite-distance
    /// domain; the captured root includes unfinished split siblings.
    pub(crate) fn nearest_candidates(&self, query: Bounds, domain: Bounds, budget: &Arc<MemoryBudget>) -> Result<Option<Nearest>> {
        let _pin = self.pin();
        Nearest::candidates(self.clone(), query, domain, budget)
    }

    /// Candidates farthest first, each with its distance's negated upper
    /// bound, nondecreasing, under the certificate `nearest_candidates`
    /// takes (RULES.md D135).
    pub(crate) fn farthest_candidates(&self, query: Bounds, domain: Bounds, budget: &Arc<MemoryBudget>) -> Result<Option<Nearest>> {
        let _pin = self.pin();
        Nearest::farthest(self.clone(), query, domain, budget)
    }

    /// Whether `query` and every entry lie within `domain`, as the
    /// captured root's envelope and its unfinished split siblings show:
    /// the certificate `nearest_candidates` takes, for a distance window.
    /// Ancestors are enlarged before an entry is inserted, so it covers
    /// every entry committed before the call.
    ///
    /// # Errors
    ///
    /// Storage errors or malformed links.
    pub(crate) fn within_domain(&self, query: Bounds, domain: Bounds) -> Result<bool> {
        if !domain.contains(query) {
            return Ok(false);
        }
        let _pin = self.pin();
        let envelope = nearest::root_envelope(self, self.root()?)?;
        Ok(envelope.is_none_or(|bounds| domain.contains(bounds)))
    }

    /// Streams intersecting pairs, pruning disjoint branches of both
    /// trees. Self-joins include both directions and each self-pair.
    /// Stable pairs appear once; concurrent changes are optional.
    ///
    /// # Errors
    ///
    /// Storage errors; SQLCODE -918 if the frontier exceeds the budget.
    /// An error after construction is yielded once by the iterator.
    pub fn join(&self, other: &RTree, budget: &Arc<MemoryBudget>) -> Result<Join> {
        let _pin = self.pin();
        Join::new(self.clone(), other.clone(), budget)
    }

    /// Frees the nodes and meta page of a discarded tree. The caller must
    /// ensure no operation, including a cursor, will access it again.
    ///
    /// # Errors
    ///
    /// Storage errors or malformed links. Some pages may already be freed.
    pub fn free(&self) -> Result<()> {
        let mut pages = Vec::new();
        let mut walk = scan::Walk::new(self.clone(), None);
        while let Some((id, _)) = walk.node()? {
            pages.push(id);
        }
        drop(walk);
        pages.extend(self.inner.epochs.all_retired());
        pages.push(self.inner.meta);
        pages.into_iter().try_for_each(|id| self.inner.pool.free(self.inner.file, id))
    }

    fn root(&self) -> Result<Visit> {
        let meta = self.meta_fields()?;
        // The root's level is in its header; its entries are read by
        // whoever visits it.
        let level = Node::level_of(&self.inner.pool.read(self.inner.file, meta.root.0)?)?;
        Ok(Visit { id: meta.root, stamp: meta.stamp, level })
    }

    fn meta_fields(&self) -> Result<Meta> {
        let page = self.inner.pool.read(self.inner.file, self.inner.meta.0)?;
        let mut r = Reader::new(&page, "R-tree metadata");
        let tag = r.u8()?;
        let version = r.u8()?;
        if tag != META_TAG || version != META_VERSION {
            return Err(r.corrupt("its tag or format version is unsupported"));
        }
        let meta = Meta { root: PageId(r.u64()?), stamp: r.u64()?, next: r.u64()? };
        r.finish()?;
        Ok(meta)
    }

    /// A split stamp no node of the tree has had, from the meta page's
    /// counter, so a page freed and allocated again repeats no stamp a
    /// scan remembers (RULES.md D139). Taken after the node's own lock.
    fn fresh_stamp(&self) -> Result<u64> {
        let _meta = self.inner.locks.lock(self.inner.meta)?;
        let mut meta = self.meta_fields()?;
        let stamp = meta.next;
        meta.next = stamp.checked_add(1).ok_or_else(|| corrupt("its split stamps are exhausted"))?;
        *self.inner.pool.write(self.inner.file, self.inner.meta)? = meta_bytes(meta);
        Ok(stamp)
    }

    fn read(&self, id: PageId) -> Result<Node> {
        let mut node = Node::empty(0, 0);
        self.read_into(id, &mut node)?;
        Ok(node)
    }

    /// Reads node `id` into `node`, reusing its entries' allocation.
    fn read_into(&self, id: PageId, node: &mut Node) -> Result<()> {
        let page = self.inner.pool.read(self.inner.file, id.0)?;
        node.parse_into(&page)?;
        if node.right == Some(id) || node.entries.len() > self.inner.capacity {
            return Err(corrupt("a node links to itself or exceeds its capacity"));
        }
        Ok(())
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

fn meta_bytes(meta: Meta) -> Vec<u8> {
    let mut bytes = vec![META_TAG, META_VERSION];
    bytes.put_u64(meta.root.0);
    bytes.put_u64(meta.stamp);
    bytes.put_u64(meta.next);
    bytes
}

/// The pages taken out and not yet freed are freed as the last clone
/// goes, since no operation can hold them then. A failure leaves them to the
/// file, which costs space only.
impl Drop for Inner {
    fn drop(&mut self) {
        for page in self.epochs.all_retired() {
            let _ = self.pool.free(self.file, page);
        }
    }
}

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] an R-tree is malformed: {why}"))
}

#[cfg(test)]
mod tests;
