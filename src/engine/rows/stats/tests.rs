//! Unit tests of the row store's statistics (`stats.rs`): a large table
//! sampled, a small one read whole, and the runs chosen with the chance
//! they should be (RULES.md R-TEST-1).

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
/// second that modulo 10, its third that divided by 64, each value of
/// the third held by rows next to each other.
fn table_of(rows: u64) -> RowTable {
    let fs = MemoryFileSystem::new();
    let crc = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Arc::new(Pager::create(&fs, std::path::Path::new("/rows"), crc, Pipeline::none(), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(1024, Box::new(Clock::new(1024))).unwrap());
    let file = pool.add_pager(pager);
    let int = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    let table = RowTable::create(
        &TableFiles::new(pool, file, Arc::new(MemoryLog::new())),
        &[int.clone(), int.clone(), int],
        TableOptions::default(),
    )
    .unwrap();
    for n in 0..rows {
        let value = |v: u64| Value::Exact(Decimal::new(i128::from(v), 0));
        table.insert(NO_TRANSACTION, vec![value(n), value(n % 10), value(n.div_euclid(64))]).unwrap();
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
    // A value of the third is in one run of row ids alone: counted as
    // seen once, the distinct values scale to the table's.
    let clustered = stats.columns[2].distinct / f64::from(u32::try_from(rows.div_euclid(64)).unwrap());
    assert!((clustered - 1.0_f64).abs() < 0.25_f64, "{} distinct of clustered values", stats.columns[2].distinct);
    assert!(stats.sample.as_ref().is_some_and(|s| s.rows().len() == crate::stats::sample::SAMPLE_ROWS));
    let small = table_of(1_000).gather().unwrap();
    assert_eq!(small.rows, 1_000);
    assert!(!small.columns[0].unbounded && !small.columns[1].unbounded, "read whole");
    assert_eq!(small.sample.as_ref().map(|s| s.rows().len()), Some(1_000));
}

/// Verifies: [DESIGN] RULES.md D1092, first statistics: a table made
/// has none until a pass is made, as before; a table opened gathers
/// them on the thread of the plan that first asks, which is given
/// them at once, and those asking after are given the same, no pass
/// made again. MC/DC of the pass made by the asking: a table opened
/// with none (made) against one not opened, and one with some.
#[test]
fn a_table_opened_gathers_its_first_statistics_as_a_plan_asks() {
    let table = table_of(1_000);
    assert!(table.statistics().is_none(), "a table made");
    assert_eq!(table.gathered.passes(), 0);
    table.gathered.gather_when_asked();
    let stats = table.statistics().expect("gathered by the asking");
    assert!(stats.rows().is_some_and(|rows| (rows - 1_000.0_f64).abs() < 0.5_f64));
    assert_eq!(table.gathered.passes(), 1);
    assert!(table.statistics().is_some());
    assert_eq!(table.gathered.passes(), 1, "made once");
}

/// Verifies: [DESIGN] RULES.md D381: a pass notes of each column at
/// how many passes in a row its greatest value rose, so a column
/// growing with insertion is known after two, and one that did not
/// rise is not; a pass finding the same greatest value keeps the
/// count; the statistics read after rows are written answer
/// by the change, a range past the greatest value keeping the share
/// written since.
#[test]
fn a_column_rising_at_each_pass_is_known_to_grow() {
    let table = table_of(1_000);
    let value = |v: u64| Value::Exact(Decimal::new(i128::from(v), 0));
    let first = table.gather().unwrap();
    assert_eq!(first.columns[0].rises, 0);
    for round in 1..=2_u64 {
        for n in 0..100 {
            table.insert(NO_TRANSACTION, vec![value(round * 10_000 + n), value(n % 10), value(0)]).unwrap();
        }
        let stats = table.gather().unwrap();
        assert_eq!(stats.columns[0].rises, u8::try_from(round).unwrap());
        assert_eq!(stats.columns[1].rises, 0, "its greatest value did not rise");
    }
    assert_eq!(table.gather().unwrap().columns[0].rises, 2, "a pass finding the same greatest keeps the count");
    for n in 0..120 {
        table.insert(NO_TRANSACTION, vec![value(30_000 + n), value(1), value(0)]).unwrap();
    }
    let read = table.statistics().unwrap();
    let past = read.selectivity(0, crate::value::CompOp::Gt, &value(25_000)).unwrap();
    assert!((past - 0.1_f64).abs() < 0.001_f64, "the share written since: {past}");
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

/// Verifies: [DESIGN] RULES.md D407, a table changed past the share
/// of its rows is due, but its pass waits for the spacing after the
/// last; changed past its rows, and REFRESH_ROWS more, it does not;
/// once the spacing is past it may be made. MC/DC of the spacing:
/// waited (made) against not waited (waits), changes past the rows
/// (made) against not past.
#[test]
fn a_pass_waits_its_spacing_unless_the_table_changed_past_its_rows() {
    let gathered = super::Gathered::new(super::Refresh::default());
    gathered.rows.store(1_000, std::sync::atomic::Ordering::Relaxed);
    gathered.earliest.store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
    gathered.changed(200);
    assert!(gathered.due() && !gathered.spaced(), "past the share, not spaced");
    gathered.changed(900);
    assert!(gathered.spaced(), "past the rows and REFRESH_ROWS");
    let fresh = super::Gathered::new(super::Refresh::default());
    fresh.rows.store(1_000, std::sync::atomic::Ordering::Relaxed);
    fresh.changed(200);
    assert!(fresh.due() && fresh.spaced(), "spaced");
    fresh.spaced_after(std::time::Duration::from_secs(3_600));
    assert!(fresh.due() && !fresh.spaced(), "after a pass of an hour");
}

/// Verifies: [DESIGN] RULES.md D473, statistics on demand: a table
/// changed past the share of its rows whose statistics no plan read
/// is not gathered by the maintenance job, though asked to now it is;
/// read by a plan, its next due pass is made, and the pass clears the
/// mark. MC/DC of the gate: read (made) against not read (not made),
/// `now` (made) against not.
#[test]
fn statistics_no_plan_reads_are_not_gathered() {
    let table = table_of(1_000);
    table.gather().unwrap();
    // Spaced from the last pass at once: only the gate holds a pass back.
    let spaced = || table.gathered.earliest.store(0, std::sync::atomic::Ordering::Relaxed);
    spaced();
    table.gathered.changed(500);
    assert!(!table.gather_wanted() && !table.gather_if_due(false), "no plan read them");
    assert!(table.statistics().is_some());
    assert!(table.gather_wanted() && table.gather_if_due(false), "read since the last pass");
    spaced();
    table.gathered.changed(500);
    assert!(!table.gather_if_due(false), "the pass cleared the mark");
    assert!(table.gather_if_due(true), "asked to now");
}

/// Verifies: [DESIGN] RULES.md D473: a planning reading statistics
/// marks them read once, and plans reading them again leave the mark
/// set; a pass clears it and the next reading sets it again.
#[test]
fn statistics_read_again_keep_their_mark() {
    let table = table_of(100);
    table.gather().unwrap();
    let read = || table.gathered.read.load(std::sync::atomic::Ordering::Acquire);
    assert!(!read(), "cleared by the pass");
    for _ in 0..3_u8 {
        assert!(table.statistics().is_some());
        assert!(read(), "read since the pass");
    }
    table.gather().unwrap();
    assert!(!read());
    assert!(table.statistics().is_some() && read());
}
