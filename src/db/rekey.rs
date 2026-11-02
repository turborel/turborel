//! \[DESIGN\] The sweep writing a database's files again in the formats
//! its settings name now (RULES.md D48): once STORAGE.ENCODINGS or
//! STORAGE.CHECKSUM_KEY changes, every file writes in the new formats
//! from then on (`Database::tell_formats`), and what was written before
//! is read as it was; so the data file holds pages in the old formats
//! until each is written again, and the old key cannot be retired. The
//! job `turborel-rekey` (`db/background.rs`) looks at the data file's
//! pages in runs, from the first to the last, and has each page still
//! in other formats written again as it is (`BufferPool::rewrite_behind`),
//! at the pace STORAGE.REKEY_BYTES_PER_SECOND sets, a tenth of a second's
//! worth a run, a page at least, so the database's own work is slowed
//! little; 0 pauses it.
//! The walk starts again from the first page whenever the formats change
//! under it. At its end the data file is settled: its header written
//! again naming the formats every page is in (`BufferPool::settle_formats`),
//! and a checkpoint taken, which writes the pages held back, commits the
//! shadowed data file, and truncates the log's segments before it; a
//! second if a segment in the old formats held the first's position. The
//! sweep is needed again, after a crash or a change, while the header
//! names other formats than the settings do, so it resumes across opens
//! with nothing recorded but the header.
//!
//! Once the data file is settled and no log segment or catalog file is
//! in other formats, the files are settled: told once as
//! `Event::FormatsSettled`, after the checkpoint that made it so, and
//! shown in INFORMATION_SCHEMA.FILE_FORMATS (`system/formats.rs`) with
//! what is still behind. The old key, or encoding, may then leave the
//! keyring or the registrations. Not swept: a plug-in's own files under
//! `plugins/`, a replica's journal. `Database::rekey` does the sweep now,
//! for `turboreld rekey` and tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::stores::CatalogStore;
use super::{Database, State};
use crate::diagnostics::Event;
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::Formats;
use crate::error::Result;
use crate::settings::{ENCODINGS, REKEY_BYTES_PER_SECOND};
use crate::sync::lock_builtin;
use crate::threads::Turn;
use crate::value::Value;

/// \[DESIGN\] The runs the sweep makes a second at most: each a tenth of
/// STORAGE.REKEY_BYTES_PER_SECOND, so the pool is never asked for more
/// at once; and the time between two.
const RUNS_PER_SECOND: u64 = 10;
const BETWEEN_RUNS: Duration = Duration::from_millis(100);

/// \[DESIGN\] What the sweep keeps between its runs: the data file and
/// the catalog store, once the database is opened from its files, and
/// the walk under way.
pub(crate) struct Rekey {
    data: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    catalog: Mutex<Option<Arc<dyn CatalogStore>>>,
    walk: Mutex<Walk>,
    /// Whether the files settling is to be told, once they are: set as the
    /// data file is found behind, cleared as they are told settled.
    untold: AtomicBool,
    /// Taken while a run is made, so runs take turns.
    turn: Turn,
}

/// The walk over the data file: the formats it writes pages again in,
/// and the page it looks at next.
#[derive(Default)]
struct Walk {
    target: Option<Formats>,
    next: u64,
}

/// \[DESIGN\] A kind of file of the database, and how much of it is
/// still in other formats than the settings name (RULES.md D48).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatsBehind {
    /// DATA, LOG or CATALOG.
    pub kind: &'static str,
    /// Of DATA, the pages the sweep has not looked at since the formats
    /// changed; of LOG, the segments in other formats; of CATALOG, 1 if
    /// its file is in other formats.
    pub behind: u64,
    /// Whether the file is wholly in the formats the settings name.
    pub settled: bool,
}

impl Default for Rekey {
    fn default() -> Rekey {
        Rekey {
            data: Mutex::new(None),
            catalog: Mutex::new(None),
            walk: Mutex::default(),
            untold: AtomicBool::new(false),
            turn: Turn::default(),
        }
    }
}

impl Rekey {
    /// The files the sweep works on, as the database is opened from them.
    pub(super) fn attach(&self, data: Option<(Arc<BufferPool>, FileId)>, catalog: Arc<dyn CatalogStore>) {
        *lock_builtin(&self.data) = data;
        *lock_builtin(&self.catalog) = Some(catalog);
        *lock_builtin(&self.walk) = Walk::default();
        self.untold.store(false, Ordering::Release);
    }

    /// The data file, if the database is opened from its files.
    fn data(&self) -> Option<(Arc<BufferPool>, FileId)> {
        lock_builtin(&self.data).clone()
    }

    /// Whether the data file's header names other formats than its pages
    /// take now: the sweep is needed.
    fn data_behind(&self) -> Result<bool> {
        let Some((pool, file)) = self.data() else { return Ok(false) };
        let (current, settled) = pool.formats_of(file)?;
        Ok(current != settled)
    }

    /// The walk's next page, the walk started again if the formats are
    /// not those it was writing pages again in.
    fn resume(&self, current: &Formats) -> u64 {
        let mut walk = lock_builtin(&self.walk);
        if walk.target.as_ref() != Some(current) {
            *walk = Walk { target: Some(current.clone()), next: 0 };
            self.untold.store(true, Ordering::Release);
        }
        walk.next
    }
}

impl State {
    /// \[DESIGN\] Each kind of file of the database kept in a directory,
    /// and how much of it is still in other formats than the settings
    /// name (RULES.md D48): none of a database kept in memory.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the data file is not registered with its pool.
    pub(crate) fn formats_behind(&self) -> Result<Vec<FormatsBehind>> {
        let Some((pool, file)) = self.rekey.data() else { return Ok(Vec::new()) };
        let (current, settled) = pool.formats_of(file)?;
        let pages = if current == settled {
            0
        } else {
            let walk = lock_builtin(&self.rekey.walk);
            let next = if walk.target.as_ref() == Some(&current) { walk.next } else { 0 };
            pool.page_count(file)?.0.saturating_sub(next)
        };
        let segments = self.log.as_ref().map_or(0, |log| log.formats_behind());
        let files = lock_builtin(&self.rekey.catalog).as_ref().map_or(0, |catalog| catalog.formats_behind());
        Ok(vec![
            FormatsBehind { kind: "DATA", behind: pages, settled: current == settled },
            FormatsBehind { kind: "LOG", behind: segments, settled: segments == 0 },
            FormatsBehind { kind: "CATALOG", behind: files, settled: files == 0 },
        ])
    }
}

/// The sweep's work as a job: one run, and when to run again: a tenth of
/// a second later while pages are left to look at; not until asked once
/// none is, or STORAGE.REKEY_BYTES_PER_SECOND is 0.
///
/// # Errors
///
/// The run's.
pub(super) fn rekeyed(db: &Database) -> Result<Option<Duration>> {
    let st = db.snapshot()?;
    let _turn = st.rekey.turn.take("the sweep")?;
    Ok(db.rekey_run(&st)?.then_some(BETWEEN_RUNS))
}

impl Database {
    /// \[DESIGN\] Writes the data file's pages still in other formats
    /// than STORAGE.ENCODINGS and STORAGE.CHECKSUM_KEY name again, now,
    /// until none is left, and settles the file in them with a checkpoint
    /// (see `db/rekey.rs`): what `turborel-rekey` does in its own time.
    /// Nothing of a database kept in memory, or settled already.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed, or this is called from
    /// within the sweep; the errors of reading or writing its pages, or
    /// of the checkpoint.
    pub fn rekey(&self) -> Result<()> {
        let st = self.snapshot()?;
        let _turn = st.rekey.turn.take("the sweep")?;
        while self.rekey_run(&st)? {}
        Ok(())
    }

    /// One run of the sweep: whether another is to follow.
    fn rekey_run(&self, st: &State) -> Result<bool> {
        let Some((pool, file)) = st.rekey.data() else { return Ok(false) };
        if !st.rekey.data_behind()? {
            return Ok(false);
        }
        let (current, _) = pool.formats_of(file)?;
        let from = st.rekey.resume(&current);
        let rate = u64::try_from(st.settings.integer(REKEY_BYTES_PER_SECOND)).unwrap_or_default();
        if rate == 0 {
            return Ok(false);
        }
        let (pages, page_size) = pool.page_count(file)?;
        let page = u64::try_from(page_size).unwrap_or(u64::MAX);
        let count = rate.checked_div(page.max(1)).unwrap_or_default().checked_div(RUNS_PER_SECOND).unwrap_or_default().max(1);
        let (next, _) = pool.rewrite_behind(file, from, count)?;
        lock_builtin(&st.rekey.walk).next = next;
        if next < pages {
            return Ok(true);
        }
        if pool.settle_formats(file, &current)? {
            self.checkpoint()?;
            // The log's segment in the old formats, if one holds the
            // position the checkpoint began at: begun again before the
            // checkpoint's own record, it goes with the next.
            if st.log.as_ref().is_some_and(|log| log.formats_behind() > 0) {
                self.checkpoint()?;
            }
        }
        Ok(false)
    }

    /// Asks for the sweep if the data file is in other formats than the
    /// settings name, as the database is opened or the formats change
    /// (`Database::tell_formats`).
    pub(super) fn rekey_if_behind(&self) -> Result<()> {
        let st = self.snapshot()?;
        if st.rekey.data_behind()? {
            st.rekey.untold.store(true, Ordering::Release);
            self.request_rekey();
        }
        Ok(())
    }

    /// Tells `Event::FormatsSettled` once every file is in the formats
    /// the settings name, after the checkpoint that made it so: once, until
    /// they change again.
    pub(super) fn tell_if_settled(&self) {
        let Ok(st) = self.snapshot() else { return };
        if !st.rekey.untold.load(Ordering::Acquire) {
            return;
        }
        let Ok(behind) = st.formats_behind() else { return };
        if behind.is_empty() || behind.iter().any(|b| !b.settled) {
            return;
        }
        if st.rekey.untold.swap(false, Ordering::AcqRel) {
            let Some((pool, file)) = st.rekey.data() else { return };
            let Ok((current, _)) = pool.formats_of(file) else { return };
            let encodings = if let Value::Char(s) = st.settings.get(ENCODINGS) { s.clone() } else { String::new() };
            self.emit(&Event::FormatsSettled { encodings, checksum_key: current.key });
        }
    }
}
