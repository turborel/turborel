//! \[DESIGN\] The row store's statistics (RULES.md D31): gathered in one
//! pass over the newest committed version of every row, decoded whole, its
//! width the bytes of its record in the leaf, and over the frozen rows no
//! entry of the tree stands for. A table of more than twice `SAMPLED_ROWS`
//! rows is sampled instead: each run of `CHUNK_IDS` row ids is read with a
//! chance of `SAMPLED_ROWS` in its rows, as a hash of the run with a fixed
//! seed decides, so a pass reads about `SAMPLED_ROWS` rows however large
//! the table, and the same rows each time; its rows are then the count the
//! table keeps (`rows/counted.rs`), and the rest is scaled from the sample
//! (`StatsBuilder::finish_sampled`). Of its frozen rows only those of the
//! runs sampled are read, and only their groups' headers
//! (`frozen/sampled.rs`). A table opened knows its count of rows
//! as it opens (`rows/counted.rs`), and has no statistics until first asked for, once recovery is
//! done (`rows/recovery.rs`): the plan that first asks gathers them itself,
//! a pass of at most twice `SAMPLED_ROWS` rows whatever the table holds,
//! and plans asking meanwhile wait for that pass, so none is made without
//! them and the pass is made once (RULES.md D1092). A table opened whose
//! statistics a checkpoint saved has those from the first, and makes no
//! such pass (`stats/saved.rs`, RULES.md D1093): it gathers them again as
//! a table that changed since does, its rows then against its rows now
//! the changes it knows of. If the first pass fails
//! the statistics are due, and the database's maintenance thread gathers them
//! (`TableStorage::maintain`), the planner meanwhile estimating without.
//! After that, they are gathered again,
//! by the database's maintenance thread, once a
//! transaction that wrote the table ends and the table has had more
//! changes since the last pass than a share of its rows then, and a number
//! more, the table's options STATISTICS.REFRESH_FRACTION and REFRESH_ROWS
//! (`rows/options.rs`): each pass then follows changes in proportion to the
//! rows it reads, so keeping them costs a bounded share of the writes. A
//! pass also waits `PASS_SPACING` times as long as the last one took, so
//! a table written fast costs at most a bounded share of one thread's time
//! (RULES.md D407); one whose changes outnumber the rows of the last pass,
//! and REFRESH_ROWS more, is not made to wait. The thread gathers them
//! only if a planning read them since the last pass began, so statistics
//! no plan reads cost nothing to keep (RULES.md D473). One thread gathers
//! at a time; another finding them due meanwhile leaves them.
//!
//! A sampled pass tells the builder which run of row ids each row is of,
//! so a value seen in one run alone counts as seen once (RULES.md D381).
//! Each pass notes, of each column, whether its greatest value rose past
//! the last pass's, so a column growing with insertion is known; and the
//! planner reads the statistics told how many rows were written since
//! and how many the table holds now (`stats/since.rs`).

mod image;
mod saved;

use std::ops::Bound;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use super::RowTable;
use super::frozen::Chosen;
use super::key;
use super::mvcc::Reader;
use super::options::{self, Refresh};
use super::row_id;
use crate::catalog::TableDesc;
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::stats::parts::count;
use crate::stats::since::{Changed, GROWING};
use crate::stats::{Statistics, StatsBuilder, TableStats};
use crate::sync::{lock_builtin, read, write};
use crate::txn::{NO_TRANSACTION, TxId};
use crate::value::compare;
pub(super) use saved::Kept;

/// \[DESIGN\] The rows a pass over a large table reads, about: three times
/// the values a column's sample keeps (`parts::SAMPLE_VALUES`), so the
/// sample is drawn from rows spread over the table, not from its first.
const SAMPLED_ROWS: u64 = 30_000;
/// \[DESIGN\] The row ids read together by a sampled pass: a run of them
/// lies in a leaf or two, so one descent of the tree reads it.
const CHUNK_IDS: u64 = 128;
/// The seed of the hash choosing the runs a sampled pass reads.
const SAMPLE_SEED: u64 = 0x2545_F491_4F6C_DD1D;
/// \[DESIGN\] How many times as long as a pass took the next waits after
/// it, at least: a table's passes then take at most a fiftieth of one
/// thread's time (RULES.md D407).
const PASS_SPACING: u32 = 50;

/// Whether a sampled pass of a table of `rows` rows reads the run of row
/// ids `chunk`: with a chance of `SAMPLED_ROWS` in `rows`, by a hash of
/// the run (XXH3-64 of its number, RULES.md D460), the same each time.
fn sampled(chunk: u64, rows: u64) -> bool {
    let z = crate::hash::xxhash::Words::new(SAMPLE_SEED).hash(chunk);
    z.checked_rem(rows).is_none_or(|r| r < SAMPLED_ROWS)
}

/// A run of row ids a pass reads, by their keys: from, and up to.
type Run = (Bound<Vec<u8>>, Bound<Vec<u8>>);

/// The runs of row ids a pass reads of the tree: every one, or of a
/// sampled pass those numbered `chunks`.
fn runs_of(chunks: Option<&[u64]>) -> Vec<Run> {
    let Some(chunks) = chunks else { return vec![(Bound::Unbounded, Bound::Unbounded)] };
    let run = |c: u64| (key(c.saturating_mul(CHUNK_IDS)).to_vec(), key(c.saturating_add(1).saturating_mul(CHUNK_IDS)).to_vec());
    chunks.iter().map(|&c| run(c)).map(|(low, high)| (Bound::Included(low), Bound::Excluded(high))).collect()
}

/// \[DESIGN\] A table's statistics, and how much it has changed since.
pub(super) struct Gathered {
    /// How they are sampled and refreshed: the store's, and the table's own.
    base: Refresh,
    refresh: RwLock<Refresh>,
    stats: RwLock<Option<Arc<TableStats>>>,
    /// Rows inserted, updated or deleted by what has committed since the
    /// last pass, which saw them. Not counted as they are written: a pass
    /// cannot see an open transaction's, and would take them as seen.
    changes: AtomicU64,
    /// The rows the last pass found.
    rows: AtomicU64,
    /// The passes that gathered them (RULES.md D991).
    passes: AtomicU64,
    /// Set while a thread gathers them.
    gathering: AtomicBool,
    /// Set for a table opened: due when first asked for.
    when_asked: AtomicBool,
    /// Set as a table opened is first asked for them, until gathered.
    asked: AtomicBool,
    /// Held by the plan making a table opened's first pass, the plans
    /// asking meanwhile waiting on it: whether that pass was tried.
    first: Mutex<bool>,
    /// Set as a planning reads them, until a pass begins: the maintenance
    /// job gathers only statistics read since its last pass (RULES.md D473).
    read: AtomicBool,
    /// When it was made, which `earliest` counts from.
    made: Instant,
    /// The nanoseconds after `made` before which the next pass waits, of
    /// changes no more than the last pass's rows (`PASS_SPACING`).
    earliest: AtomicU64,
    /// Where they are saved with the table's pages (`stats/saved.rs`), of
    /// a table that has pages.
    kept: Option<Arc<Kept>>,
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
            passes: AtomicU64::default(),
            gathering: AtomicBool::default(),
            when_asked: AtomicBool::default(),
            asked: AtomicBool::default(),
            first: Mutex::default(),
            read: AtomicBool::default(),
            made: Instant::now(),
            earliest: AtomicU64::default(),
            kept: None,
        }
    }

    /// Itself, its statistics saved by `kept`.
    pub(super) fn kept_by(mut self, kept: Arc<Kept>) -> Gathered {
        self.kept = Some(kept);
        self
    }

    /// Makes its statistics saved as `sealing` seals.
    pub(super) fn seal_with(&self, sealing: &Arc<super::sealing::Sealing>) {
        if let Some(kept) = &self.kept {
            kept.seal_with(sealing);
        }
    }

    /// The page its statistics are saved in, 0 for none.
    pub(super) fn page(&self) -> u64 {
        self.kept.as_ref().map_or(0, |kept| kept.page())
    }

    /// Frees the page its statistics are saved in, of a table dropped.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub(super) fn free_page(&self) -> Result<()> {
        self.kept.as_ref().map_or(Ok(()), |kept| kept.free_page())
    }

    /// Takes the statistics page `page` holds, of a table opened that
    /// holds `live` rows, if it holds any: the rows it has more or fewer
    /// than they found are changes since their pass, so a table that
    /// changed much after its last pass gathers them again as it would
    /// have.
    pub(super) fn restore(&self, page: u64, live: u64) {
        let Some(stats) = self.kept.as_ref().and_then(|kept| kept.load(page)) else { return };
        let Ok(mut held) = write(&self.stats) else { return };
        self.rows.store(stats.rows, Ordering::Relaxed);
        self.changes.store(live.abs_diff(stats.rows), Ordering::Relaxed);
        *held = Some(Arc::new(stats));
    }

    /// Keeps `stats`, a pass's, as the table's, to be saved by the next
    /// checkpoint.
    fn keep(&self, stats: &Arc<TableStats>) -> Result<()> {
        *write(&self.stats)? = Some(Arc::clone(stats));
        if let Some(kept) = &self.kept {
            kept.wait(stats);
        }
        Ok(())
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

    /// Counts `rows` inserted, updated or deleted, committed.
    pub(super) fn changed(&self, rows: u64) {
        self.changes.fetch_add(rows, Ordering::Relaxed);
    }

    /// Whether a table opened was asked for them, or the table has changed
    /// enough to gather them again: past the share of its rows.
    fn due(&self) -> bool {
        let Refresh { fraction, rows: more, .. } = self.refresh();
        let rows = count(self.rows.load(Ordering::Relaxed));
        self.asked.load(Ordering::Acquire) || count(self.changes.load(Ordering::Relaxed)) > rows * fraction + count(more)
    }

    /// Whether a plan read them since the last pass began, or a table
    /// opened was asked for them: only then does the maintenance job
    /// gather them.
    fn wanted(&self) -> bool {
        self.asked.load(Ordering::Acquire) || self.read.load(Ordering::Acquire)
    }

    /// Whether a pass due may be made now: asked for, the last pass long
    /// enough past, or the table changed past its rows, and REFRESH_ROWS
    /// more.
    fn spaced(&self) -> bool {
        let rows = count(self.rows.load(Ordering::Relaxed));
        let past = count(self.changes.load(Ordering::Relaxed)) > rows + count(self.refresh().rows);
        self.asked.load(Ordering::Acquire) || past || self.since_made() >= self.earliest.load(Ordering::Relaxed)
    }

    /// The nanoseconds since it was made.
    fn since_made(&self) -> u64 {
        u64::try_from(self.made.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// Makes the next pass wait, after one that took `took`, `PASS_SPACING`
    /// times as long as it took.
    fn spaced_after(&self, took: Duration) {
        let wait = u64::try_from(took.saturating_mul(PASS_SPACING).as_nanos()).unwrap_or(u64::MAX);
        self.earliest.store(self.since_made().saturating_add(wait), Ordering::Relaxed);
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

    /// The passes that gathered them.
    pub(super) fn passes(&self) -> u64 {
        self.passes.load(Ordering::Acquire)
    }

    /// The statistics of the last pass, if there was one.
    pub(super) fn get(&self) -> Option<Arc<TableStats>> {
        read(&self.stats).ok()?.clone()
    }

    /// The statistics of the last pass, as `asked_for` gives them, read
    /// after the rows written since, the table holding `live` rows.
    pub(super) fn answer(&self, live: u64) -> Option<Arc<dyn Statistics>> {
        // Written only as it changes, so plans reading them at once share
        // the line and do not take it from each other's cores.
        if !self.read.load(Ordering::Relaxed) {
            self.read.store(true, Ordering::Release);
        }
        let stats = self.asked_for()?;
        Some(Changed::of(stats, self.changes.load(Ordering::Relaxed), live))
    }
}

/// Notes in `stats` of each column at how many passes in a row its
/// greatest value rose past `previous`'s, the last pass's. A pass that
/// finds the same greatest value keeps the count, as one soon after
/// another, by maintenance and then by ANALYZE, says nothing of growth.
fn risen(stats: &mut TableStats, previous: Option<&TableStats>) {
    let Some(previous) = previous else { return };
    for (column, before) in stats.columns.iter_mut().zip(&previous.columns) {
        let order = column.max.as_ref().zip(before.max.as_ref()).and_then(|(now, then)| compare(now, then));
        column.rises = match order {
            Some(std::cmp::Ordering::Greater) => before.rises.saturating_add(1).min(GROWING),
            Some(std::cmp::Ordering::Equal) => before.rises,
            Some(std::cmp::Ordering::Less) | None => 0,
        };
    }
}

impl RowTable {
    /// Gathers the table's statistics in one pass, of every row or, of a
    /// large table, a sample of them (see the module's documentation), and
    /// keeps them.
    ///
    /// # Errors
    ///
    /// A row cannot be read; the statistics are left as they were.
    pub(super) fn gather(&self) -> Result<Arc<TableStats>> {
        // Changes made during the pass may or may not be seen: they count
        // towards the next.
        let begun = Instant::now();
        // Read during the pass, they are wanted by the next.
        self.gathered.read.store(false, Ordering::Release);
        let before = self.gathered.changes.load(Ordering::Relaxed);
        let rows = u64::try_from(self.counted.live.load(Ordering::Acquire)).unwrap_or(u64::MAX);
        let sampling = rows > SAMPLED_ROWS.saturating_mul(2);
        let (columns, how) = (self.format.columns(), self.gathered.refresh().sampling);
        let mut builder = if sampling { StatsBuilder::counting(columns, how) } else { StatsBuilder::sampling(columns, how) };
        // The runs of row ids a sampled pass reads, of the tree and of
        // the groups alike.
        let chunks = sampling.then(|| self.sampled_chunks(rows));
        // Its groups as they are before the tree is read, held to the end.
        let directory = self.frozen.directory()?;
        let runs = runs_of(chunks.as_deref());
        for (low, high) in &runs {
            for entry in self.tree.scan(low.as_ref().map(Vec::as_slice), high.as_ref().map(Vec::as_slice), Direction::Forward) {
                let (key, newest) = entry?;
                // Read again if an older version is released as it is read.
                let visible = self.unpinned((&key, newest), 0, &mut |bytes, _| self.visible_from(bytes.to_vec(), Reader::Latest))?;
                let Some(record) = visible.flatten() else { continue };
                let id = row_id(&key)?;
                let row = self.decode(id, &record, None)?;
                builder.at_run(id.checked_div(CHUNK_IDS).unwrap_or(0));
                builder.add(&row, record.len());
            }
        }
        // The frozen rows no entry stands for, each as wide as its record
        // would be (`rows/frozen.rs`).
        let mut record = Vec::new();
        // Only the rows sampled are decoded.
        let chosen = chunks.as_deref().map(|numbers| Chosen { runs: numbers, ids: CHUNK_IDS });
        self.each_unshadowed(&directory, chosen, |id, row| {
            record.clear();
            self.format.encode(&row, &mut record)?;
            builder.at_run(id.checked_div(CHUNK_IDS).unwrap_or(0));
            builder.add(&row, record.len());
            Ok(())
        })?;
        let mut stats = if sampling { builder.finish_sampled(rows) } else { builder.finish() };
        risen(&mut stats, self.gathered.get().as_deref());
        let stats = Arc::new(stats);
        self.gathered.keep(&stats)?;
        self.gathered.rows.store(stats.rows, Ordering::Relaxed);
        self.gathered.changes.fetch_sub(before, Ordering::Relaxed);
        self.gathered.asked.store(false, Ordering::Release);
        self.gathered.spaced_after(begun.elapsed());
        self.gathered.passes.fetch_add(1, Ordering::Release);
        Ok(stats)
    }

    /// The numbers of the runs of `CHUNK_IDS` row ids a pass sampling a
    /// table of `rows` rows reads, those `sampled` chooses, in order.
    fn sampled_chunks(&self, rows: u64) -> Vec<u64> {
        let ends = self.next.load(Ordering::Acquire).div_ceil(CHUNK_IDS);
        (0..ends).filter(|&chunk| sampled(chunk, rows)).collect()
    }

    /// Counts a row written by `tx`: at once outside a transaction, whose
    /// writes are committed as they are made; else at its commit
    /// (`commit_open`).
    pub(super) fn counted_change(&self, tx: TxId) {
        if tx == NO_TRANSACTION {
            self.gathered.changed(1);
        }
    }

    /// Whether its statistics are due to be gathered again, by the
    /// database's maintenance thread (`TableStorage::maintain`): changed
    /// enough, and read by a plan since the last pass (RULES.md D473).
    pub(super) fn gather_wanted(&self) -> bool {
        self.gathered.due() && self.gathered.wanted()
    }

    /// Gathers the first statistics of a table opened, on the thread of
    /// the plan first asking for them, those asking meanwhile waiting
    /// until they are gathered (RULES.md D1092): tried once, a failure
    /// leaving them due for the maintenance thread.
    fn gather_first(&self) {
        let gathered = &self.gathered;
        if !gathered.when_asked.load(Ordering::Relaxed) || gathered.get().is_some() {
            return;
        }
        let mut tried = lock_builtin(&gathered.first);
        if *tried || gathered.get().is_some() {
            return;
        }
        *tried = true;
        // The maintenance thread at the same pass makes it instead.
        if !gathered.gathering.swap(true, Ordering::Acquire) {
            // A failure leaves them due when asked for (`asked_for`).
            let _ = self.gather();
            gathered.gathering.store(false, Ordering::Release);
        }
    }

    /// Its statistics, as a plan asks for them (`TableStorage::statistics`):
    /// a table opened's first gathered by the asking.
    pub(super) fn statistics_asked(&self) -> Option<Arc<dyn Statistics>> {
        self.gather_first();
        self.gathered.answer(u64::try_from(self.counted.live.load(Ordering::Acquire)).unwrap_or(u64::MAX))
    }

    /// Gathers them if due and asked to `now`, or wanted and spaced from
    /// the last pass, and no other thread is: whether it did. A failure
    /// leaves them for the next call.
    pub(super) fn gather_if_due(&self, now: bool) -> bool {
        let gathered = &self.gathered;
        let due = gathered.due() && (now || (gathered.wanted() && gathered.spaced()));
        if due && !self.gathered.gathering.swap(true, Ordering::Acquire) {
            let _ = self.gather();
            self.gathered.gathering.store(false, Ordering::Release);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests;
