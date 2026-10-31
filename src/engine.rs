//! \[DESIGN\] The built-in storage engines (RULES.md D31): a row store and a
//! column store, which freezes rows written to the row store into groups of
//! each column's values, over the storage foundation (D30). Built bottom up:
//!
//! * `record`: a row's bytes, by its columns' types, any column found
//!   without decoding the rest;
//! * `key`: index keys as bytes that sort as SQL compares their values;
//! * `tree`: a B-link tree over the buffer pool, keyed by bytes, for
//!   tables (by row id) and indexes (by key, then row id);
//! * `hash`: a hash table over the buffer pool, by linear hashing, for the
//!   hash index method and operators that spill;
//! * `rows`: the row-store layout, a table as a tree of records keyed by
//!   row id, long values out of line;
//! * `columns`: the column layout's frozen row groups, each column's
//!   values in chunks of their own, under a table of the row store.

pub mod columns;
pub mod hash;
pub mod key;
pub mod record;
pub mod rows;
pub mod tree;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::disk::buffer::{BufferPool, FileId, READ_AHEAD_PAGES};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::{DEFAULT_PAGE_SIZE, Pager};
use crate::error::SqlError;
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::memory::MemoryBudget;
use crate::sync::lock_builtin;
use crate::value::DataType;

/// \[DESIGN\] The bytes of the buffer pool of a pager file in memory, 2
/// MiB: its pages are in memory anyway, so the pool only spares their
/// encoding and checksum.
pub const MEMORY_POOL_BYTES: usize = 2 << 20;

/// A pager file in a file system of its own in memory, in a buffer pool of
/// its own, both drawing on `budget`, held by `what`: where the built-in engine
/// and indexes of a database kept nowhere else put their pages.
///
/// # Errors
///
/// SQLCODE -918 if the budget cannot grant the pool; the pager's errors.
#[cfg(test)]
pub(crate) fn memory_file(budget: &Arc<MemoryBudget>, what: &str) -> Result<(Arc<BufferPool>, FileId), SqlError> {
    memory_file_of(budget, MEMORY_POOL_BYTES, what)
}

/// As `memory_file`, its pool of `bytes`.
fn memory_file_of(budget: &Arc<MemoryBudget>, bytes: usize, what: &str) -> Result<(Arc<BufferPool>, FileId), SqlError> {
    let fs = MemoryFileSystem::within(Arc::clone(budget), &format!("the pages of {what}"));
    let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI)?);
    let pager = Pager::create(&fs, Path::new("/pages"), hash, Pipeline::none(), DEFAULT_PAGE_SIZE)?;
    let pool = Arc::new(BufferPool::in_budget(budget, (bytes, pager.page_size()), what)?);
    let file = pool.add_pager(Arc::new(pager));
    Ok((pool, file))
}

/// \[DESIGN\] Pages in memory, made when first asked for: a pager file in a
/// file system of its own, in a buffer pool of the bytes it is sized to
/// then, reserved of the budget it is given. A database kept in memory
/// gives one to its built-in engine and index methods, which share it, as
/// a database kept in a directory shares its data file, sized by its
/// setting STORAGE.POOL_BYTES, its read-ahead by STORAGE.READ_AHEAD_PAGES.
pub(crate) struct MemoryPages {
    budget: Mutex<Arc<MemoryBudget>>,
    bytes: AtomicUsize,
    /// The pages its pool reads ahead at once (`BufferPool::read_ahead`).
    read_ahead: AtomicUsize,
    made: Mutex<Option<(Arc<BufferPool>, FileId, usize)>>,
}

impl MemoryPages {
    /// Pages whose pool will be of `bytes`, reserved of a budget of their
    /// own, limited by nothing else, until they are given another.
    pub(crate) fn new(bytes: usize) -> Arc<MemoryPages> {
        Arc::new(MemoryPages {
            budget: Mutex::new(MemoryBudget::new(usize::MAX)),
            bytes: AtomicUsize::new(bytes),
            read_ahead: AtomicUsize::new(READ_AHEAD_PAGES),
            made: Mutex::new(None),
        })
    }

    /// Makes `budget` the one their pool draws on, if made after.
    pub(crate) fn use_budget(&self, budget: Arc<MemoryBudget>) {
        *lock_builtin(&self.budget) = budget;
    }

    /// Makes their pool of `bytes`, if made after.
    pub(crate) fn resize(&self, bytes: usize) {
        self.bytes.store(bytes, Ordering::Release);
    }

    /// Has their pool read `pages` ahead at once, made or when it is.
    pub(crate) fn set_read_ahead(&self, pages: usize) {
        let made = lock_builtin(&self.made);
        self.read_ahead.store(pages, Ordering::Release);
        if let Some((pool, _, _)) = made.as_ref() {
            pool.set_read_ahead(pages);
        }
    }

    /// Their pool and file, made if not yet, held by `what`.
    ///
    /// # Errors
    ///
    /// As `memory_file`.
    pub(crate) fn file(&self, what: &str) -> Result<(Arc<BufferPool>, FileId), SqlError> {
        let mut made = lock_builtin(&self.made);
        if let Some((pool, file, _)) = made.as_ref() {
            return Ok((Arc::clone(pool), *file));
        }
        let budget = Arc::clone(&lock_builtin(&self.budget));
        let bytes = self.bytes.load(Ordering::Acquire);
        let (pool, file) = memory_file_of(&budget, bytes, what)?;
        pool.set_read_ahead(self.read_ahead.load(Ordering::Acquire));
        *made = Some((Arc::clone(&pool), file, bytes));
        Ok((pool, file))
    }

    /// The bytes their pool was made of, if it was made.
    pub(crate) fn made_bytes(&self) -> Option<usize> {
        lock_builtin(&self.made).as_ref().map(|(_, _, bytes)| *bytes)
    }
}

/// The width of a column's value in a record's fixed part, if it has one,
/// as its kind says: character strings are variable.
pub(crate) fn fixed_width(data_type: DataType) -> Option<usize> {
    data_type.kind().record_width(&data_type)
}
