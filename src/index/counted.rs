//! \[DESIGN\] What the label and reachability index methods share
//! (RULES.md D115, D116): their pager file, made in memory for a method
//! with none, and a B-link tree of counted entries. An entry's value
//! counts the versions of its row holding it (D29), four bytes, so each
//! insert is taken back by one remove (D34), changed in one descent under
//! its leaf's lock (`BLinkTree::update`).
//!
//! ```text
//! locator: version u8, layout u32, the tree's meta page u64
//! ```

use std::sync::{Arc, Mutex};

use crate::bytes::{Put, Reader};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::disk::pager::PageId;
use crate::engine::MemoryPages;
use crate::engine::tree::{BLinkTree, Update};
use crate::error::{Code, Result, SqlError};
use crate::memory::Host;
use crate::sync::lock_builtin;

/// Bytes of a count of versions.
const VERSIONS_BYTES: usize = 4;
/// The format of a locator.
const LOCATOR_VERSION: u8 = 1;
/// Bytes of a locator: its version, its layout and its tree's meta page.
const LOCATOR_BYTES: usize = 1 + 4 + 8;

/// \[DESIGN\] Where a method's indexes are: a pager file of a pool, or a
/// pool of its own in memory, made with its first index.
pub(super) struct Files {
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    memory: Arc<MemoryPages>,
}

impl Files {
    /// Pager file `file` of `pool`.
    pub(super) fn new(pool: Arc<BufferPool>, file: FileId) -> Files {
        Files { files: Mutex::new(Some((pool, file))), memory: MemoryPages::new(0) }
    }

    /// Pages in `memory`, which others may share.
    pub(super) fn sharing(memory: Arc<MemoryPages>) -> Files {
        Files { files: Mutex::new(None), memory }
    }

    /// Its pool in memory draws on `host`'s budget.
    pub(super) fn attach(&self, host: &Host) {
        self.memory.use_budget(Arc::clone(&host.memory));
    }

    /// Its file, made in memory if it has none yet.
    ///
    /// # Errors
    ///
    /// The pool in memory cannot be made.
    pub(super) fn get(&self) -> Result<(Arc<BufferPool>, FileId)> {
        let mut files = lock_builtin(&self.files);
        if let Some((pool, file)) = files.as_ref() {
            return Ok((Arc::clone(pool), *file));
        }
        let (pool, file) = self.memory.file("the pool in memory of the built-in engine and index methods")?;
        *files = Some((Arc::clone(&pool), file));
        Ok((pool, file))
    }

    /// A new, empty tree in its file.
    ///
    /// # Errors
    ///
    /// The file's.
    pub(super) fn tree(&self) -> Result<BLinkTree> {
        let (pool, file) = self.get()?;
        BLinkTree::create(pool, file)
    }

    /// The tree of meta page `meta` in its file.
    ///
    /// # Errors
    ///
    /// The file's.
    pub(super) fn opened(&self, meta: u64) -> Result<BLinkTree> {
        let (pool, file) = self.get()?;
        BLinkTree::open(pool, file, PageId(meta))
    }

    /// Writes back every page and syncs the file.
    ///
    /// # Errors
    ///
    /// The file's.
    pub(super) fn checkpoint(&self) -> Result<()> {
        let (pool, file) = self.get()?;
        pool.sync(file, Durability::Full)
    }
}

/// The error of an entry not of its index's format.
pub(super) fn corrupt(what: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] {what} index's entry is not of its layout"))
}

/// The versions a value counts.
fn versions_of(value: &[u8]) -> Result<u32> {
    let bytes: [u8; VERSIONS_BYTES] = value.try_into().map_err(|_| corrupt("a graph"))?;
    Ok(u32::from_be_bytes(bytes))
}

/// The versions of the entry `value` holds, of the tree's scan.
///
/// # Errors
///
/// SQLCODE -916 if it is not a count.
pub(super) fn counted(value: &[u8]) -> Result<u32> {
    versions_of(value)
}

/// Adds a version of entry `key` to `tree`: whether it is its first.
///
/// # Errors
///
/// The tree's; SQLCODE -900 if the entry is longer than a node holds.
pub(super) fn add(tree: &BLinkTree, key: &[u8]) -> Result<bool> {
    let most = tree.max_key();
    if key.len() > most {
        return Err(SqlError::new(
            Code::Unsupported,
            format!(
                "[DESIGN] an index entry of {} bytes is longer than the limit of {most} bytes for this database's page size; a database created with a larger STORAGE.PAGE_BYTES holds longer entries",
                key.len()
            ),
        ));
    }
    let mut first = false;
    tree.update(key, |old| {
        let n = old.map(versions_of).transpose()?.unwrap_or(0);
        first = n == 0;
        Ok(Update::Put(n.saturating_add(1).to_be_bytes().to_vec()))
    })?;
    Ok(first)
}

/// Takes back a version of entry `key` of `tree`, if it has one: whether
/// one was, and whether it was the last.
///
/// # Errors
///
/// The tree's.
pub(super) fn take(tree: &BLinkTree, key: &[u8]) -> Result<(bool, bool)> {
    let (mut taken, mut last) = (false, false);
    tree.update(key, |old| {
        let Some(n) = old.map(versions_of).transpose()? else { return Ok(Update::Keep) };
        (taken, last) = (true, n <= 1);
        Ok(if last { Update::Remove } else { Update::Put(n.saturating_sub(1).to_be_bytes().to_vec()) })
    })?;
    Ok((taken, last))
}

/// The locator of an index of layout number `layout` in `tree`.
pub(super) fn locator(layout: u32, tree: &BLinkTree) -> Vec<u8> {
    let mut out = vec![LOCATOR_VERSION];
    out.put_u32(layout);
    out.put_u64(tree.meta().0);
    out
}

/// The layout number and meta page a locator holds.
///
/// # Errors
///
/// SQLCODE -916 if it is not of this format.
pub(super) fn located(locator: &[u8], what: &'static str) -> Result<(u32, u64)> {
    let mut reader = Reader::new(locator, what);
    if locator.len() != LOCATOR_BYTES || reader.u8()? != LOCATOR_VERSION {
        return Err(reader.corrupt("its version is not known"));
    }
    let layout = reader.u32()?;
    let meta = reader.u64()?;
    reader.finish()?;
    Ok((layout, meta))
}
