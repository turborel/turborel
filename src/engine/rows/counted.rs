//! \[DESIGN\] A row store table's count of rows kept with its pages
//! (RULES.md D31, D25): the rows whose newest version is not a deletion,
//! an open transaction's too, as `TableStorage::row_count` gives them.
//!
//! Each checkpoint, writers held, writes the count into the table's meta
//! page (`rows/meta.rs`) if it changed since last written, so the pages a
//! checkpoint captures hold the count of the newest versions they hold.
//! A table opened from a shadowed file (`disk/shadow.rs`), which holds
//! after a crash exactly what its last checkpoint captured, reads its
//! count there, with no pass over its rows; recovery then moves it as it
//! writes each row's newest version (`rows/recovery.rs`), by whether the
//! version it replaces was a row, so a change it writes again that the
//! pages already hold moves it not at all, and the count comes out exact.
//! A file not shadowed may hold pages written after its last checkpoint,
//! so a table opened from one counts its rows by a pass.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::meta::{meta_content, parse_meta};
use super::{TableFiles, corrupt};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::sync::lock;

/// \[DESIGN\] A table's count of rows, and where it is saved.
pub(super) struct Counted {
    pool: Arc<BufferPool>,
    file: FileId,
    meta: PageId,
    /// The rows whose newest version is not a deletion.
    pub(super) live: AtomicUsize,
    /// The count its meta page holds, under this lock while the page is
    /// written; `None` once the page is freed.
    saved: Mutex<Option<u64>>,
}

impl Counted {
    /// The count of a table whose meta page is `meta` of `file` in `pool`,
    /// holding `saved`.
    pub(super) fn new((pool, file): (&Arc<BufferPool>, FileId), meta: PageId, saved: u64) -> Counted {
        Counted { pool: Arc::clone(pool), file, meta, live: AtomicUsize::new(0), saved: Mutex::new(Some(saved)) }
    }

    /// `write` given the count the meta page holds, which stays so while
    /// it runs: for every other write of the meta page.
    ///
    /// # Errors
    ///
    /// `write`'s; SQLCODE -910 if the page is freed.
    pub(super) fn holding(&self, write: impl FnOnce(u64) -> Result<()>) -> Result<()> {
        let saved = lock(&self.saved)?;
        let rows = saved.ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a dropped table's meta page is freed"))?;
        write(rows)
    }

    /// Writes the count into the meta page, if it changed since last
    /// written.
    ///
    /// # Errors
    ///
    /// The buffer pool's: the count written stays as it was, and the next
    /// checkpoint writes it; SQLCODE -916 if the page is not a table's.
    fn save(&self) -> Result<()> {
        let mut saved = lock(&self.saved)?;
        let Some(was) = *saved else { return Ok(()) };
        let rows = u64::try_from(self.live.load(Ordering::Acquire)).unwrap_or(u64::MAX);
        if rows == was {
            return Ok(());
        }
        let mut page = self.pool.write(self.file, self.meta)?;
        let mut meta = parse_meta(&page).ok_or_else(|| corrupt(&format!("page {} is not a table's meta page", self.meta.0)))?;
        meta.rows = rows;
        *page = meta_content(meta);
        *saved = Some(rows);
        Ok(())
    }

    /// Frees the meta page, of a table dropped; saved no more.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub(super) fn free_meta(&self) -> Result<()> {
        let mut saved = lock(&self.saved)?;
        *saved = None;
        self.pool.free(self.file, self.meta)
    }
}

impl TableFiles {
    /// Saves the count of rows of each table open of its files, a part of
    /// a checkpoint (`StorageEngine::checkpoint`).
    ///
    /// # Errors
    ///
    /// The buffer pool's: the checkpoint fails, and the counts not written
    /// are written by the next.
    pub(super) fn save_counts(&self) -> Result<()> {
        let open: Vec<Arc<Counted>> = {
            let mut tables = lock(&self.counts)?;
            tables.retain(|c| c.strong_count() > 0);
            tables.iter().filter_map(Weak::upgrade).collect()
        };
        open.iter().try_for_each(|counted| counted.save())
    }

    /// Notes `counted`, of a table made or opened, to be saved at each
    /// checkpoint.
    pub(super) fn note_counted(&self, counted: &Arc<Counted>) -> Result<()> {
        lock(&self.counts)?.push(Arc::downgrade(counted));
        Ok(())
    }

    /// Whether a table opened of its files may take its count of rows from
    /// its meta page: its file holds what a checkpoint captured, no more,
    /// only if shadowed.
    pub(super) fn counts_kept(&self) -> bool {
        self.shadows.is_some()
    }

    /// Counts a pass over a table's rows as it opened.
    pub(super) fn passed(&self) {
        self.passes.fetch_add(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests;
