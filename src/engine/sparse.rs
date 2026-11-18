//! \[EXTENSION\] The sparse inverted index of the vector design's §10.1
//! ("SPARSE INVERTED: posting list per dimension with block-max WAND"):
//! for each dimension a sparse vector holds, the rows holding it and their
//! weights, in posting blocks (`block.rs`) kept in one B-link tree
//! (`engine/tree.rs`, RULES.md D31) of a pager file, as the btree index
//! keeps its own. The tree's layout is in `keys.rs`.
//!
//! * Writes (`write.rs`) are counted, as the index contract asks: a row's
//!   vector is counted under its hash, and its postings are written by
//!   the insert that finds none of them, taken back by the remove that
//!   takes its last count. One dimension's blocks are changed by one
//!   writer at a time, under the dimension's stripe lock, so a block's
//!   rows stay below the next block's key.
//! * Readers lock nothing. A reader of a dimension's blocks skips the rows
//!   at most the last row of the blocks it has passed, so a block split or
//!   merged while it reads gives none of its postings twice (`cursor.rs`).
//! * Top-k by dot product is Block-Max WAND, after Ding and Suel, "Faster
//!   Top-k Document Retrieval Using Block-Max Indexes", SIGIR 2011, over
//!   WAND, after Broder et al., "Efficient Query Evaluation using a
//!   Two-Level Retrieval Process", CIKM 2003 (`wand.rs`).
//! * A bulk load sorts postings in a work area and writes the tree whole
//!   (`load.rs`); maintenance merges blocks that removes left small and
//!   tightens dimensions' bounds (`maintain.rs`).
//!
//! The pages reach the disk as the buffer pool writes them back: in a
//! database kept in a directory, its data file, shadowed, so after a crash
//! the index is as the last checkpoint left it (RULES.md D25).

mod block;
mod cursor;
pub mod half;
mod keys;
mod load;
mod maintain;
pub mod vector;
mod wand;
mod write;

use std::collections::BTreeSet;
use std::ops::Bound;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub use block::{QUANTIZATIONS, Quantization};
pub use vector::{HASH_BYTES, SparseVector};
pub use wand::{Ask, Filter, Found, Hit, Rescore};

use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::tree::{BLinkTree, Direction, Scan, scan_of};
use crate::error::{Code, Result, SqlError};
use crate::storage::RowId;
use crate::sync::lock_builtin;

/// \[EXTENSION\] The postings of a block by default: §10.4's "per-block
/// max weight (block 128)".
pub const DEFAULT_BLOCK: usize = 128;
/// \[DESIGN\] The fewest postings a block may be set to hold: fewer makes
/// headers outweigh postings.
pub const LEAST_BLOCK: usize = 16;
/// \[DESIGN\] The most: past this, a block's decoding outweighs what its
/// bound skips.
pub const MOST_BLOCK: usize = 4_096;
/// \[DESIGN\] Stripes of dimension locks, each serializing the writers of
/// the dimensions it holds, a fixed count whatever the vocabulary.
const DIMENSION_LOCKS: u32 = 64;
/// \[DESIGN\] Stripes of row locks, serializing the counts of one row.
const ROW_LOCKS: u64 = 64;
/// \[DESIGN\] The dimensions remembered as removes leave them; past this,
/// maintenance goes over every dimension.
const MOST_DIRTY: usize = 4_096;
/// \[DESIGN\] The removes after which maintenance is due.
const MAINTAIN_AFTER: u64 = 1_024;

/// \[EXTENSION\] How an index lays out its blocks: SPARSE.QUANTIZATION and
/// SPARSE.BLOCK_SIZE. Either changes in place: blocks written after use
/// it, those before are read by their own header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// How weights are stored.
    pub quantization: Quantization,
    /// The most postings a block holds, from `LEAST_BLOCK` to `MOST_BLOCK`.
    pub block: usize,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout { quantization: Quantization::Uint8, block: DEFAULT_BLOCK }
    }
}

/// What the postings take, for the compression ratio.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Footprint {
    /// Postings.
    pub postings: u64,
    /// Blocks.
    pub blocks: u64,
    /// The bytes of the blocks' values.
    pub bytes: u64,
    /// The bytes of the postings uncompressed: a u64 row and an f32 weight.
    pub raw: u64,
}

/// \[EXTENSION\] A sparse inverted index in a pager file. Clones share it;
/// it may be shared between threads.
#[derive(Clone)]
pub struct SparseTree {
    inner: Arc<Inner>,
}

/// What removes left to maintenance.
#[derive(Default)]
struct Dirty {
    dimensions: BTreeSet<u32>,
    /// Too many to remember: every dimension.
    all: bool,
}

struct Inner {
    tree: Arc<BLinkTree>,
    quantization: AtomicU8,
    block: AtomicUsize,
    dimensions: Vec<Mutex<()>>,
    rows: Vec<Mutex<()>>,
    /// The documents, if counted: none for an index opened, until
    /// maintenance counts them.
    documents: Mutex<Option<u64>>,
    dirty: Mutex<Dirty>,
    removed: AtomicU64,
}

impl SparseTree {
    /// An empty index in pager file `file` of `pool`.
    ///
    /// # Errors
    ///
    /// The tree's: SQLCODE -910 if `file` is not a registered pager file.
    pub fn create(pool: Arc<BufferPool>, file: FileId, layout: Layout) -> Result<SparseTree> {
        let tree = BLinkTree::create(pool, file)?;
        Ok(SparseTree::with(tree, layout, Some(0)))
    }

    /// The index whose tree's meta page is `meta`.
    ///
    /// # Errors
    ///
    /// The tree's: SQLCODE -916 if `meta` is not a tree's meta page.
    pub fn open(pool: Arc<BufferPool>, file: FileId, meta: PageId, layout: Layout) -> Result<SparseTree> {
        let tree = BLinkTree::open(pool, file, meta)?;
        Ok(SparseTree::with(tree, layout, None))
    }

    fn with(tree: BLinkTree, layout: Layout, documents: Option<u64>) -> SparseTree {
        let locks = |n: u64| (0..n).map(|_| Mutex::new(())).collect();
        SparseTree {
            inner: Arc::new(Inner {
                tree: Arc::new(tree),
                quantization: AtomicU8::new(layout.quantization.code()),
                block: AtomicUsize::new(layout.block.clamp(LEAST_BLOCK, MOST_BLOCK)),
                dimensions: locks(u64::from(DIMENSION_LOCKS)),
                rows: locks(ROW_LOCKS),
                documents: Mutex::new(documents),
                dirty: Mutex::new(Dirty::default()),
                removed: AtomicU64::new(0),
            }),
        }
    }

    /// Its tree's meta page, by which `open` finds it.
    pub fn meta(&self) -> PageId {
        self.inner.tree.meta()
    }

    /// Its layout now.
    pub fn layout(&self) -> Layout {
        let code = self.inner.quantization.load(Ordering::Acquire);
        let quantization = [Quantization::Float32, Quantization::Float16, Quantization::Uint8]
            .into_iter()
            .find(|q| q.code() == code)
            .unwrap_or(Quantization::Uint8);
        Layout { quantization, block: self.inner.block.load(Ordering::Acquire) }
    }

    /// Lays out blocks written from now on as `layout` says.
    pub fn set_layout(&self, layout: Layout) {
        if self.layout().quantization != layout.quantization {
            // Blocks of the old quantization are stored again by maintenance.
            lock_builtin(&self.inner.dirty).all = true;
            self.inner.removed.fetch_max(MAINTAIN_AFTER, Ordering::Relaxed);
        }
        self.inner.quantization.store(layout.quantization.code(), Ordering::Release);
        self.inner.block.store(layout.block.clamp(LEAST_BLOCK, MOST_BLOCK), Ordering::Release);
    }

    /// The vectors it holds, each (hash, row) once, if counted.
    pub fn documents(&self) -> Option<u64> {
        *lock_builtin(&self.inner.documents)
    }

    /// The (hash, row) of every vector it holds, each once, in hash order,
    /// read a leaf at a time as they are taken, holding the tree.
    pub fn entries(&self) -> impl Iterator<Item = Result<([u8; HASH_BYTES], RowId)>> + Send + use<> {
        self.scan(keys::tag_range(keys::DOCUMENT)).map(|entry| keys::document_of(&entry?.0))
    }

    /// The rows with a vector of hash `hash`, each once.
    ///
    /// # Errors
    ///
    /// The tree's.
    pub fn rows(&self, hash: &[u8; HASH_BYTES]) -> Result<Vec<RowId>> {
        self.scan(keys::hash_range(hash)).map(|entry| Ok(keys::document_of(&entry?.0)?.1)).collect()
    }

    /// What its postings take, read whole.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if a block is malformed.
    pub fn footprint(&self) -> Result<Footprint> {
        const RAW_POSTING: u64 = 12;
        let mut out = Footprint::default();
        for entry in self.scan(keys::tag_range(keys::POSTINGS)) {
            let (_, value) = entry?;
            let postings = u64::try_from(block::Header::read(&value)?.count).unwrap_or(u64::MAX);
            out.postings = out.postings.saturating_add(postings);
            out.blocks = out.blocks.saturating_add(1);
            out.bytes = out.bytes.saturating_add(u64::try_from(value.len()).unwrap_or(u64::MAX));
        }
        out.raw = out.postings.saturating_mul(RAW_POSTING);
        Ok(out)
    }

    /// Frees every page of its tree: for an index no one reads or writes
    /// again.
    ///
    /// # Errors
    ///
    /// The tree's.
    pub fn free(&self) -> Result<()> {
        self.inner.tree.free()
    }

    /// A forward scan of keys from `lower` to below `upper`, holding the tree.
    fn scan(&self, (lower, upper): (Vec<u8>, Vec<u8>)) -> Scan<Arc<BLinkTree>> {
        scan_of(Arc::clone(&self.inner.tree), Bound::Included(lower.as_slice()), Bound::Excluded(upper.as_slice()), Direction::Forward)
    }

    /// The stripe lock of `dimension`.
    fn dimension_lock(&self, dimension: u32) -> Result<std::sync::MutexGuard<'_, ()>> {
        let stripe = usize::try_from(dimension % DIMENSION_LOCKS).unwrap_or_default();
        self.inner.dimensions.get(stripe).map(lock_builtin).ok_or_else(|| missing("a dimension lock stripe"))
    }

    /// The stripe lock of `row`.
    fn row_lock(&self, row: RowId) -> Result<std::sync::MutexGuard<'_, ()>> {
        let stripe = usize::try_from(row % ROW_LOCKS).unwrap_or_default();
        self.inner.rows.get(stripe).map(lock_builtin).ok_or_else(|| missing("a row lock stripe"))
    }

    /// Notes a vector more or fewer, if they are counted.
    fn counted(&self, more: bool) {
        if let Some(n) = lock_builtin(&self.inner.documents).as_mut() {
            *n = if more { n.saturating_add(1) } else { n.saturating_sub(1) };
        }
    }

    /// Notes `dimension` as one a remove left blocks of to merge.
    fn dirtied(&self, dimension: u32) {
        self.inner.removed.fetch_add(1, Ordering::Relaxed);
        let mut dirty = lock_builtin(&self.inner.dirty);
        if dirty.dimensions.len() < MOST_DIRTY {
            dirty.dimensions.insert(dimension);
        } else {
            dirty.all = true;
        }
    }

    /// Whether maintenance has work: the documents to count, or blocks
    /// enough removes may have left small.
    pub fn maintenance_due(&self) -> bool {
        self.documents().is_none() || self.inner.removed.load(Ordering::Relaxed) >= MAINTAIN_AFTER
    }
}

fn missing(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {what} of a sparse index is missing"))
}

#[cfg(test)]
mod tests;
