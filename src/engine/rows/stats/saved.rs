//! \[DESIGN\] A row store table's statistics kept with its pages (RULES.md
//! D1093): saved at the checkpoint after each pass that gathered them, on
//! a page of their own the table's meta page names, and read as the table
//! is opened, so a table opened plans its first statement by the
//! statistics it was closed with, and gathers none until it has changed
//! enough (`rows/stats.rs`). A table that never had a pass saved has no
//! such page, and gathers its first as it always did (RULES.md D1092).
//!
//! The page is of any length (the pager's overflow) and holds what
//! `image.rs` describes. A sample of the table's rows is among it, so it
//! is sealed as the table's rows are, its page number the context, and
//! freed with the table's other pages when the table is dropped. What
//! cannot be sealed, its encodings not registered, or cannot be written
//! as records, is saved empty; and a page empty, or one that cannot be
//! opened or read, is read as no statistics, which the table then gathers
//! again. No failure of statistics fails a table's opening.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::image;
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::record::RecordFormat;
use crate::engine::rows::counted::Counted;
use crate::engine::rows::meta::{meta_content, parse_meta};
use crate::engine::rows::sealing::Sealing;
use crate::engine::rows::{TableFiles, corrupt};
use crate::error::Result;
use crate::stats::TableStats;
use crate::sync::{lock, lock_builtin};

/// The bit of the statistics page's sealing context setting it apart from
/// a row's, a frozen group's page's and a zone maps' segment's.
const STATISTICS: u64 = 1 << 61;

/// \[DESIGN\] Where a table's statistics are saved, and those a pass
/// gathered that no checkpoint has saved yet.
pub(in crate::engine::rows) struct Kept {
    pool: Arc<BufferPool>,
    file: FileId,
    /// The table's meta page, which names the statistics page.
    meta: PageId,
    /// The table's count of rows, which every write of the meta page holds.
    counted: Arc<Counted>,
    format: RecordFormat,
    /// How the table's rows are sealed.
    sealing: Mutex<Arc<Sealing>>,
    /// The statistics page, 0 while none is made; none once the table is
    /// dropped and the page freed.
    place: Mutex<Option<u64>>,
    /// The statistics page as the meta page names it, 0 for none.
    named: AtomicU64,
    /// The statistics of the last pass, until a checkpoint saves them.
    waiting: Mutex<Option<Arc<TableStats>>>,
}

impl TableFiles {
    /// Saves the statistics gathered since the last checkpoint of each
    /// table open of its files, a part of a checkpoint
    /// (`StorageEngine::checkpoint`).
    ///
    /// # Errors
    ///
    /// The buffer pool's: the checkpoint fails, and the statistics not
    /// saved are saved by the next.
    pub(in crate::engine::rows) fn save_statistics(&self) -> Result<()> {
        let open: Vec<Arc<Kept>> = {
            let mut tables = lock(&self.statistics)?;
            tables.retain(|k| k.strong_count() > 0);
            tables.iter().filter_map(Weak::upgrade).collect()
        };
        open.iter().try_for_each(|kept| kept.save())
    }

    /// Notes `kept`, of a table made or opened, to be saved at each
    /// checkpoint.
    pub(in crate::engine::rows) fn note_statistics(&self, kept: &Arc<Kept>) -> Result<()> {
        lock(&self.statistics)?.push(Arc::downgrade(kept));
        Ok(())
    }
}

impl Kept {
    /// The place of the statistics of the table of meta page `meta` in
    /// `files`, its records of `format`, its count of rows `counted`:
    /// no page yet, sealed plainly until told (`seal_with`).
    pub(in crate::engine::rows) fn new(files: &TableFiles, meta: PageId, counted: &Arc<Counted>, format: &RecordFormat) -> Kept {
        Kept {
            pool: Arc::clone(&files.pool),
            file: files.file,
            meta,
            counted: Arc::clone(counted),
            format: format.clone(),
            sealing: Mutex::new(Arc::new(Sealing::plain())),
            place: Mutex::new(Some(0)),
            named: AtomicU64::new(0),
            waiting: Mutex::default(),
        }
    }

    /// Makes its statistics saved as `sealing` seals.
    pub(in crate::engine::rows) fn seal_with(&self, sealing: &Arc<Sealing>) {
        *lock_builtin(&self.sealing) = Arc::clone(sealing);
    }

    /// The statistics page the meta page names, 0 for none.
    pub(in crate::engine::rows) fn page(&self) -> u64 {
        self.named.load(Ordering::Acquire)
    }

    /// Keeps `stats`, a pass's, to be saved by the next checkpoint.
    pub(in crate::engine::rows) fn wait(&self, stats: &Arc<TableStats>) {
        *lock_builtin(&self.waiting) = Some(Arc::clone(stats));
    }

    /// The statistics page `page` holds, as the table's meta page names
    /// it, of a table opened: none if it names none, or the page is
    /// empty, or cannot be opened or read.
    pub(in crate::engine::rows) fn load(&self, page: u64) -> Option<TableStats> {
        *lock_builtin(&self.place) = Some(page);
        self.named.store(page, Ordering::Release);
        if page == 0 {
            return None;
        }
        let stored = self.pool.read(self.file, page).ok()?;
        if stored.is_empty() {
            return None;
        }
        let sealing = Arc::clone(&lock_builtin(&self.sealing));
        let content = sealing.plain_value(page | STATISTICS, &stored).ok()?;
        image::decoded(&content, &self.format).ok()
    }

    /// Saves the statistics waiting, if any are.
    fn save(&self) -> Result<()> {
        let Some(stats) = lock_builtin(&self.waiting).take() else { return Ok(()) };
        let saved = self.save_of(&stats);
        if saved.is_err() {
            // Saved by the next checkpoint, unless a pass since left newer.
            lock_builtin(&self.waiting).get_or_insert(stats);
        }
        saved
    }

    /// `save`'s work, of `stats`: nothing of a table dropped.
    fn save_of(&self, stats: &TableStats) -> Result<()> {
        let mut place = lock(&self.place)?;
        let Some(at) = place.as_mut() else { return Ok(()) };
        if *at == 0 {
            *at = self.made()?;
        }
        let sealing = Arc::clone(&lock_builtin(&self.sealing));
        let mut stored = Vec::new();
        let context = *at | STATISTICS;
        // Not known after a restart if they cannot be written or sealed.
        let whole = image::encoded(stats, &self.format).and_then(|content| sealing.value(context, &content, &mut stored));
        if whole.is_err() {
            stored.clear();
        }
        *self.pool.write(self.file, PageId(*at))? = stored;
        Ok(())
    }

    /// A new page for the statistics, named by the table's meta page.
    fn made(&self) -> Result<u64> {
        let page = self.pool.new_page(self.file)?.0;
        let named = self.counted.holding(|_| {
            let mut content = self.pool.write(self.file, self.meta)?;
            let mut meta = parse_meta(&content).ok_or_else(|| corrupt(&format!("page {} is not a table's meta page", self.meta.0)))?;
            meta.statistics = page.0;
            *content = meta_content(meta);
            self.named.store(page.0, Ordering::Release);
            Ok(())
        });
        if named.is_err() {
            // Lost to the file if it cannot be freed, which costs space only.
            let _ = self.pool.free(self.file, page);
        }
        named.map(|()| page.0)
    }

    /// Frees the page its statistics are saved in, of a table dropped;
    /// saved no more.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub(in crate::engine::rows) fn free_page(&self) -> Result<()> {
        let Some(page) = lock(&self.place)?.take() else { return Ok(()) };
        lock_builtin(&self.waiting).take();
        if page != 0 {
            self.pool.free(self.file, PageId(page))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
