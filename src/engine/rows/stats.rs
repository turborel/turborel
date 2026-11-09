//! \[DESIGN\] The row store's statistics (RULES.md D31): gathered in one
//! pass over the newest committed version of every row, decoded whole, its
//! width the bytes of its record in the leaf, and over the frozen rows no
//! entry of the tree stands for. A table of more than twice `SAMPLED_ROWS`
//! rows is sampled instead: each run of `CHUNK_IDS` row ids is read with a
//! chance of `SAMPLED_ROWS` in its rows, as a hash of the run with a fixed
//! seed decides, so a pass reads about `SAMPLED_ROWS` rows however large
//! the table, and the same rows each time; its rows are then the count the
//! table keeps (`rows/counted.rs`), and the rest is scaled from the sample
//! (`StatsBuilder::finish_sampled`). Its frozen rows are each looked at,
//! those of runs not sampled left out. A table opened knows its count of rows
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
use super::key;
use super::mvcc::Reader;
use super::options::{self, Refresh};
use super::row_id;
use crate::catalog::TableDesc;
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::stats::parts::count;
use crate::stats::{StatsBuilder, TableStats};
use crate::sync::{read, write};
use crate::txn::{NO_TRANSACTION, TxId};

/// \[DESIGN\] The rows a pass over a large table reads, about: as many as
/// PostgreSQL's ANALYZE samples at its default statistics target, three
/// times the values a column's sample keeps (`parts::SAMPLE_VALUES`).
const SAMPLED_ROWS: u64 = 30_000;
/// \[DESIGN\] The row ids read together by a sampled pass: a run of them
/// lies in a leaf or two, so one descent of the tree reads it.
const CHUNK_IDS: u64 = 128;
/// The seed of the hash choosing the runs a sampled pass reads.
const SAMPLE_SEED: u64 = 0x2545_F491_4F6C_DD1D;

/// Whether a sampled pass of a table of `rows` rows reads the run of row
/// ids `chunk`: with a chance of `SAMPLED_ROWS` in `rows`, by a hash of
/// the run (SplitMix64's finalizer), the same each time.
fn sampled(chunk: u64, rows: u64) -> bool {
    let mut z = chunk ^ SAMPLE_SEED;
    z = (z ^ (z >> 30_u32)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27_u32)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31_u32;
    z.checked_rem(rows).is_none_or(|r| r < SAMPLED_ROWS)
}

/// A run of row ids a pass reads, by their keys: from, and up to.
type Run = (Bound<Vec<u8>>, Bound<Vec<u8>>);

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

    /// Counts `rows` inserted, updated or deleted, committed.
    pub(super) fn changed(&self, rows: u64) {
        self.changes.fetch_add(rows, Ordering::Relaxed);
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
        let before = self.gathered.changes.load(Ordering::Relaxed);
        let rows = u64::try_from(self.counted.live.load(Ordering::Acquire)).unwrap_or(u64::MAX);
        let sampling = rows > SAMPLED_ROWS.saturating_mul(2);
        let (columns, how) = (self.format.columns(), self.gathered.refresh().sampling);
        let mut builder = if sampling { StatsBuilder::counting(columns, how) } else { StatsBuilder::sampling(columns, how) };
        let read = |id: u64| !sampling || sampled(id.checked_div(CHUNK_IDS).unwrap_or(0), rows);
        // Its groups as they are before the tree is read, held to the end.
        let directory = self.frozen.directory()?;
        let runs = self.runs(sampling.then_some(rows));
        for (low, high) in &runs {
            for entry in self.tree.scan(low.as_ref().map(Vec::as_slice), high.as_ref().map(Vec::as_slice), Direction::Forward) {
                let (key, newest) = entry?;
                // Read again if an older version is released as it is read.
                let visible = self.unpinned((&key, newest), 0, &mut |bytes, _| self.visible_from(bytes, Reader::Latest))?;
                let Some(record) = visible.flatten() else { continue };
                let row = self.decode(row_id(&key)?, &record, None)?;
                builder.add(&row, record.len());
            }
        }
        // The frozen rows no entry stands for, each as wide as its record
        // would be (`rows/frozen.rs`).
        let mut record = Vec::new();
        self.each_unshadowed(&directory, |id, row| {
            if read(id) {
                record.clear();
                self.format.encode(&row, &mut record)?;
                builder.add(&row, record.len());
            }
            Ok(())
        })?;
        let stats = Arc::new(if sampling { builder.finish_sampled(rows) } else { builder.finish() });
        *write(&self.gathered.stats)? = Some(Arc::clone(&stats));
        self.gathered.rows.store(stats.rows, Ordering::Relaxed);
        self.gathered.changes.fetch_sub(before, Ordering::Relaxed);
        self.gathered.asked.store(false, Ordering::Release);
        Ok(stats)
    }

    /// The runs of row ids a pass reads: every one, or of a pass sampling
    /// a table of `sampled` rows, those `sampled` chooses.
    fn runs(&self, sampled_of: Option<u64>) -> Vec<Run> {
        let Some(rows) = sampled_of else { return vec![(Bound::Unbounded, Bound::Unbounded)] };
        let ends = self.next.load(Ordering::Acquire).div_ceil(CHUNK_IDS);
        let run = |c: u64| (key(c.saturating_mul(CHUNK_IDS)).to_vec(), key(c.saturating_add(1).saturating_mul(CHUNK_IDS)).to_vec());
        let taken = (0..ends).filter(|&chunk| sampled(chunk, rows));
        taken.map(run).map(|(low, high)| (Bound::Included(low), Bound::Excluded(high))).collect()
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

// Unit tests: a large table sampled, a small one read whole, and the runs
// chosen with the chance they should be (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{CHUNK_IDS, SAMPLED_ROWS, sampled};
    use crate::disk::buffer::{BufferPool, Clock};
    use crate::disk::encoding::Pipeline;
    use crate::disk::memory::MemoryFileSystem;
    use crate::disk::pager::Pager;
    use crate::engine::rows::{RowTable, TableFiles, TableOptions};
    use crate::hash::crc::{CRC32_ISCSI, Crc};
    use crate::log::MemoryLog;
    use crate::storage::TableStorage;
    use crate::txn::NO_TRANSACTION;
    use crate::value::{DataType, Decimal, Spelled, Value};

    /// A table of `rows` rows, its first column 0 to `rows - 1`, its
    /// second that modulo 10.
    fn table_of(rows: u64) -> RowTable {
        let fs = MemoryFileSystem::new();
        let crc = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
        let pager = Arc::new(Pager::create(&fs, std::path::Path::new("/rows"), crc, Pipeline::none(), 4096).unwrap());
        let pool = Arc::new(BufferPool::new(1024, Box::new(Clock::new(1024))).unwrap());
        let file = pool.add_pager(pager);
        let int = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
        let table =
            RowTable::create(&TableFiles::new(pool, file, Arc::new(MemoryLog::new())), &[int, int], TableOptions::default()).unwrap();
        for n in 0..rows {
            let value = |v: u64| Value::Exact(Decimal::new(i128::from(v), 0));
            table.insert(NO_TRANSACTION, vec![value(n), value(n % 10)]).unwrap();
        }
        table
    }

    /// Verifies: [DESIGN] RULES.md D31, statistics: a table of more than
    /// twice `SAMPLED_ROWS` rows is sampled, its rows the count it keeps,
    /// its distinct values scaled from the sample's, a column of every
    /// value distinct near its rows and one of ten values ten, and its
    /// columns not bounded by the sample's least and greatest; a smaller
    /// table is read whole, bounded. MC/DC of the choice: rows past twice
    /// `SAMPLED_ROWS` (sampled) against rows at most that (read whole).
    #[test]
    fn a_large_table_is_sampled_and_a_small_one_read_whole() {
        let rows = SAMPLED_ROWS * 2 + 10_000;
        let stats = table_of(rows).gather().unwrap();
        assert_eq!(stats.rows, rows);
        let (all, ten) = (&stats.columns[0], &stats.columns[1]);
        let near = (all.distinct / 70_000.0_f64 - 1.0_f64).abs();
        assert!(near < 0.1_f64, "{} distinct of every value", all.distinct);
        assert!((ten.distinct - 10.0_f64).abs() < 0.5_f64, "{} distinct of ten", ten.distinct);
        assert!(all.unbounded && ten.unbounded);
        let small = table_of(1_000).gather().unwrap();
        assert_eq!(small.rows, 1_000);
        assert!(!small.columns[0].unbounded && !small.columns[1].unbounded, "read whole");
    }

    /// Verifies: [DESIGN] RULES.md D31, statistics: a sampled pass reads
    /// each run of row ids with a chance of `SAMPLED_ROWS` in the table's
    /// rows, the same runs each time, so it reads about `SAMPLED_ROWS`
    /// rows however large the table.
    #[test]
    fn a_sampled_pass_reads_runs_with_a_fixed_chance() {
        for rows in [300_000_u64, 3_000_000] {
            let runs = rows.div_euclid(CHUNK_IDS);
            let taken = (0..runs).filter(|&chunk| sampled(chunk, rows)).count();
            let read = u64::try_from(taken).unwrap() * CHUNK_IDS;
            let share = f64::from(u32::try_from(read).unwrap()) / 30_000.0_f64;
            assert!((share - 1.0_f64).abs() < 0.15_f64, "{read} rows read of {rows}");
            assert_eq!(taken, (0..runs).filter(|&chunk| sampled(chunk, rows)).count(), "the same runs");
        }
        assert!((0..100).all(|chunk| sampled(chunk, SAMPLED_ROWS)), "a table no larger than the sample: every run");
    }
}
