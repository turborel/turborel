//! \[DESIGN\] The built-in spill provider (`spill.rs`): an area is a temporary
//! pager file of its own in a buffer pool of the area's size, reserved of
//! the memory budget, so its pages are written to the file as the pool
//! evicts them, through the encodings its temporary files are given
//! (`TempFiles::pipeline`). Its tables are linear hash tables (`engine/hash.rs`); a
//! value too long for one of their entries is kept in a page of its own,
//! the entry holding its page. Its runs are pages of records, each page a
//! batch of records written at once. The file is removed when the area and
//! everything made in it are dropped. Split from spill.rs (RULES.md
//! R-SIZE-2).

use std::path::PathBuf;
use std::sync::Arc;

use super::{Area, Records, Spill, SpillArea, SpillRun, SpillTable, TempFiles};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::{DEFAULT_PAGE_SIZE, PageId, Pager};
use crate::engine::hash::HashTable;
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::memory::MemoryBudget;

/// The tag of a table's value held in its entry, and of one in a page of
/// its own.
const INLINE: u8 = 0;
const PAGED: u8 = 1;

/// \[DESIGN\] The built-in spill provider: areas in temporary pager files.
#[derive(Debug, Default)]
pub struct PagedSpill;

impl Spill for PagedSpill {
    fn area(&self, memory: &Arc<MemoryBudget>, temp: &Arc<TempFiles>, area: Area) -> Result<Box<dyn SpillArea>> {
        let path = temp.path()?;
        let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI)?);
        let pager = Pager::create_scratch(&**temp.fs(), &path, hash, temp.pipeline(), DEFAULT_PAGE_SIZE)?;
        let page_size = pager.page_size();
        let made = BufferPool::in_budget(memory, (area.bytes, page_size), "a spill area").map(Arc::new);
        let pool = match made {
            Ok(pool) => pool,
            Err(e) => {
                drop(pager);
                // The file is new and holds nothing; a failure to remove it,
                // told, leaves an empty file in the temporary directory.
                temp.fs().discard(&path);
                return Err(e);
            }
        };
        let file = pool.add_pager(Arc::new(pager));
        Ok(Box::new(PagedArea(area.batch, Arc::new(File { pool: Some(pool), id: file, path, temp: Arc::clone(temp) }))))
    }
}

/// An area's file, shared by the area and what is made in it, removed when
/// the last of them goes.
struct File {
    pool: Option<Arc<BufferPool>>,
    id: FileId,
    path: PathBuf,
    temp: Arc<TempFiles>,
}

impl File {
    fn pool(&self) -> Result<&Arc<BufferPool>> {
        self.pool.as_ref().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a spill area's pool is gone"))
    }
}

impl Drop for File {
    fn drop(&mut self) {
        // The pool, and with it the pager's file, closed first.
        drop(self.pool.take());
        // Nothing reads the file again; a failure to remove it, told,
        // leaves it in the temporary directory, cleared when the database
        // opens or closes again.
        self.temp.fs().discard(&self.path);
    }
}

/// An area: the batch of its runs, and its file.
struct PagedArea(usize, Arc<File>);

impl SpillArea for PagedArea {
    fn table(&self) -> Result<Box<dyn SpillTable>> {
        let table = Arc::new(HashTable::create(Arc::clone(self.1.pool()?), self.1.id)?);
        Ok(Box::new(PagedTable { file: Arc::clone(&self.1), table }))
    }

    fn run(&self) -> Result<Box<dyn SpillRun>> {
        Ok(Box::new(PagedRun { file: Arc::clone(&self.1), pages: Vec::new(), batch: Vec::new(), limit: self.0 }))
    }
}

/// A table of an area: shared with the reads of a key's values
/// (`SpillTable::values`).
struct PagedTable {
    file: Arc<File>,
    table: Arc<HashTable>,
}

impl PagedTable {
    /// What an entry holds of `value`: the value, or the page holding it.
    fn stored(&self, key: &[u8], value: &[u8]) -> Result<Vec<u8>> {
        if key.len().saturating_add(value.len()).saturating_add(1) <= self.table.max_entry() {
            return Ok([&[INLINE], value].concat());
        }
        let (id, mut page) = self.file.pool()?.new_page(self.file.id)?;
        *page = value.to_vec();
        Ok([&[PAGED][..], &id.0.to_le_bytes()].concat())
    }

    /// The value an entry holds.
    fn value(&self, entry: &[u8]) -> Result<Vec<u8>> {
        value_of(&self.file, entry)
    }
}

/// The value an entry of a table of `file` holds.
fn value_of(file: &File, entry: &[u8]) -> Result<Vec<u8>> {
    match entry.split_first() {
        Some((&INLINE, value)) => Ok(value.to_vec()),
        Some((&PAGED, id)) => {
            let id = <[u8; 8]>::try_from(id).map(u64::from_le_bytes).map_err(|_| corrupt())?;
            Ok(file.pool()?.read(file.id, id)?.to_vec())
        }
        Some(_) | None => Err(corrupt()),
    }
}

fn corrupt() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] a spilled entry is malformed")
}

impl SpillTable for PagedTable {
    fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        let stored = self.stored(key, value)?;
        self.table.insert(key, &stored)
    }

    fn get(&self, key: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.table.get(key)?.iter().map(|e| self.value(e)).collect()
    }

    /// Read a page of the key's bucket at a time, a value kept in a page of
    /// its own as it is taken.
    fn values(&self, key: &[u8]) -> Result<Records> {
        let file = Arc::clone(&self.file);
        Ok(Box::new(self.table.values(key).map(move |entry| entry.and_then(|e| value_of(&file, &e)))))
    }

    fn remove(&mut self, key: &[u8], value: &[u8]) -> Result<bool> {
        for entry in self.table.get(key)? {
            if self.value(&entry)? == value {
                if let Some((&PAGED, id)) = entry.split_first() {
                    let id = <[u8; 8]>::try_from(id).map(u64::from_le_bytes).map_err(|_| corrupt())?;
                    self.file.pool()?.free(self.file.id, PageId(id))?;
                }
                return self.table.remove(key, &entry);
            }
        }
        Ok(false)
    }
}

/// A run of an area: its pages, each a batch of records, and the batch
/// being gathered.
struct PagedRun {
    file: Arc<File>,
    pages: Vec<PageId>,
    batch: Vec<u8>,
    /// The bytes it gathers before it writes them as a page.
    limit: usize,
}

impl PagedRun {
    /// Writes the batch gathered as a page of its own.
    fn flush(&mut self) -> Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let (id, mut page) = self.file.pool()?.new_page(self.file.id)?;
        *page = std::mem::take(&mut self.batch);
        self.pages.push(id);
        Ok(())
    }
}

impl SpillRun for PagedRun {
    fn push(&mut self, record: &[u8]) -> Result<()> {
        self.batch.extend_from_slice(&u64::try_from(record.len()).unwrap_or(u64::MAX).to_le_bytes());
        self.batch.extend_from_slice(record);
        if self.batch.len() >= self.limit { self.flush() } else { Ok(()) }
    }

    /// Each page freed once its records are read.
    fn into_records(mut self: Box<Self>) -> Result<Records> {
        self.flush()?;
        let (file, mut pages) = (Arc::clone(&self.file), std::mem::take(&mut self.pages).into_iter());
        let mut pending: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::new();
        Ok(Box::new(std::iter::from_fn(move || {
            loop {
                if let Some(record) = pending.pop_front() {
                    return Some(Ok(record));
                }
                let id = pages.next()?;
                let read = file.pool().and_then(|pool| {
                    let page = pool.read(file.id, id.0)?.to_vec();
                    pool.free(file.id, id)?;
                    split(&page)
                });
                match read {
                    Ok(records) => pending.extend(records),
                    Err(e) => return Some(Err(e)),
                }
            }
        })))
    }

    fn records(&mut self) -> Result<Records> {
        self.flush()?;
        let (file, pages) = (Arc::clone(&self.file), self.pages.clone());
        let mut pending: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::new();
        let mut next_page = 0_usize;
        Ok(Box::new(std::iter::from_fn(move || {
            loop {
                if let Some(record) = pending.pop_front() {
                    return Some(Ok(record));
                }
                let id = *pages.get(next_page)?;
                next_page = next_page.saturating_add(1);
                match file.pool().and_then(|pool| Ok(pool.read(file.id, id.0)?.to_vec())).and_then(|page| split(&page)) {
                    Ok(records) => pending.extend(records),
                    Err(e) => return Some(Err(e)),
                }
            }
        })))
    }
}

/// The records of a run's page.
fn split(page: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut records = Vec::new();
    let mut rest = page;
    while !rest.is_empty() {
        let (len, after) = rest.split_at_checked(8).ok_or_else(corrupt)?;
        let len = <[u8; 8]>::try_from(len).map(u64::from_le_bytes).ok().and_then(|n| usize::try_from(n).ok()).ok_or_else(corrupt)?;
        let (record, next) = after.split_at_checked(len).ok_or_else(corrupt)?;
        records.push(record.to_vec());
        rest = next;
    }
    Ok(records)
}

#[cfg(test)]
mod tests;
