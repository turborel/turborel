//! \[DESIGN\] The row store's statistics (RULES.md D31): gathered in one
//! pass over the newest committed version of every row, decoded whole, its
//! width the bytes of its record in the leaf, and over the frozen rows no
//! entry of the tree stands for. A table opened knows its count of rows
//! as it opens (`rows/counted.rs`), and has no statistics until first asked for, once recovery is
//! done (`rows/recovery.rs`): then they are due, and the database's
//! maintenance thread gathers them (`TableStorage::maintain`), the planner
//! meanwhile estimating without them, so no statement waits for a pass
//! over the table, and the pass is made once, however many plan at once.
//! After that, they are gathered again,
//! by the database's maintenance thread, once a
//! transaction that wrote the table ends and the table has had more
//! changes since the last pass than a share of its rows then, and a number
//! more, the table's options STATISTICS.REFRESH_FRACTION and REFRESH_ROWS
//! (`rows/options.rs`): each pass then follows changes in proportion to the
//! rows it reads, so keeping them costs a bounded share of the writes. One thread
//! gathers at a time; another finding them due meanwhile leaves them.

use std::ops::Bound;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use super::RowTable;
use super::mvcc::Reader;
use super::options::{self, Refresh};
use super::row_id;
use crate::catalog::TableDesc;
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::stats::parts::count;
use crate::stats::{StatsBuilder, TableStats};
use crate::sync::{read, write};

/// \[DESIGN\] A table's statistics, and how much it has changed since.
pub(super) struct Gathered {
    /// How they are sampled and refreshed: the store's, and the table's own.
    base: Refresh,
    refresh: RwLock<Refresh>,
    stats: RwLock<Option<Arc<TableStats>>>,
    /// Rows inserted, updated or deleted since the last pass.
    changes: AtomicU64,
    /// The rows the last pass found.
    rows: AtomicU64,
    /// Set while a thread gathers them.
    gathering: AtomicBool,
    /// Set for a table opened: due when first asked for.
    when_asked: AtomicBool,
    /// Set as a table opened is first asked for them, until gathered.
    asked: AtomicBool,
}

impl Gathered {
    /// None yet, to be kept as `refresh` says.
    pub(super) fn new(refresh: Refresh) -> Gathered {
        Gathered {
            base: refresh,
            refresh: RwLock::new(refresh),
            stats: RwLock::default(),
            changes: AtomicU64::default(),
            rows: AtomicU64::default(),
            gathering: AtomicBool::default(),
            when_asked: AtomicBool::default(),
            asked: AtomicBool::default(),
        }
    }

    /// Keeps them as the options of `desc` say from now on.
    pub(super) fn reconfigure(&self, desc: &TableDesc) {
        if let Ok(mut refresh) = write(&self.refresh) {
            *refresh = options::refresh_of(desc, self.base);
        }
    }

    /// How they are sampled and refreshed now.
    fn refresh(&self) -> Refresh {
        read(&self.refresh).map_or(self.base, |r| *r)
    }

    /// Counts a row inserted, updated or deleted.
    pub(super) fn changed(&self) {
        self.changes.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether a table opened was asked for them, or the table has changed
    /// enough to gather them again.
    fn due(&self) -> bool {
        let Refresh { fraction, rows: more, .. } = self.refresh();
        let rows = count(self.rows.load(Ordering::Relaxed));
        self.asked.load(Ordering::Acquire) || count(self.changes.load(Ordering::Relaxed)) > rows * fraction + count(more)
    }

    /// Makes them due when first asked for.
    pub(super) fn gather_when_asked(&self) {
        self.when_asked.store(true, Ordering::Relaxed);
    }

    /// The statistics of the last pass, if there was one; if none, and the
    /// table was opened, they are due from now on.
    pub(super) fn asked_for(&self) -> Option<Arc<TableStats>> {
        let stats = self.get();
        if stats.is_none() && self.when_asked.load(Ordering::Relaxed) {
            self.asked.store(true, Ordering::Release);
        }
        stats
    }

    /// The statistics of the last pass, if there was one.
    pub(super) fn get(&self) -> Option<Arc<TableStats>> {
        read(&self.stats).ok()?.clone()
    }
}

impl RowTable {
    /// Gathers the table's statistics in one pass, and keeps them.
    ///
    /// # Errors
    ///
    /// A row cannot be read; the statistics are left as they were.
    pub(super) fn gather(&self) -> Result<Arc<TableStats>> {
        // Changes made during the pass may or may not be seen: they count
        // towards the next.
        let before = self.gathered.changes.load(Ordering::Relaxed);
        let mut builder = StatsBuilder::sampling(self.format.columns(), self.gathered.refresh().sampling);
        // Its groups as they are before the tree is read, held to the end.
        let directory = self.frozen.directory()?;
        for entry in self.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward) {
            let (key, newest) = entry?;
            // Read again if an older version is released as it is read.
            let visible = self.unpinned((&key, newest), 0, &mut |bytes, _| self.visible_from(bytes, Reader::Latest))?;
            let Some(record) = visible.flatten() else { continue };
            let row = self.decode(row_id(&key)?, &record, None)?;
            builder.add(&row, record.len());
        }
        // The frozen rows no entry stands for, each as wide as its record
        // would be (`rows/frozen.rs`).
        let mut record = Vec::new();
        self.each_unshadowed(&directory, |_, row| {
            record.clear();
            self.format.encode(&row, &mut record)?;
            builder.add(&row, record.len());
            Ok(())
        })?;
        let stats = Arc::new(builder.finish());
        *write(&self.gathered.stats)? = Some(Arc::clone(&stats));
        self.gathered.rows.store(stats.rows, Ordering::Relaxed);
        self.gathered.changes.fetch_sub(before, Ordering::Relaxed);
        self.gathered.asked.store(false, Ordering::Release);
        Ok(stats)
    }

    /// Whether its statistics are due to be gathered again, by the
    /// database's maintenance thread (`TableStorage::maintain`).
    pub(super) fn gather_wanted(&self) -> bool {
        self.gathered.due()
    }

    /// Gathers them if due and no other thread is. A failure leaves them
    /// for the next call.
    pub(super) fn gather_if_due(&self) {
        if self.gathered.due() && !self.gathered.gathering.swap(true, Ordering::Acquire) {
            let _ = self.gather();
            self.gathered.gathering.store(false, Ordering::Release);
        }
    }
}
