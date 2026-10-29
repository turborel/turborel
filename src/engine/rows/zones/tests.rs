//! Unit tests of a row store table's zone maps (`zones.rs`).

use std::sync::Arc;

use super::ZONE_ROWS;
use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::engine::rows::{RowTable, TableFiles, TableOptions};
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::index::Probe;
use crate::log::MemoryLog;
use crate::storage::{Condition, Conjunct, Conjunction, Row, RowId, TableStorage};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

/// A number kept, a long text not.
const TYPES: [DataType; 2] = [DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, DataType::Varying { length: 100 }];

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn row(n: i128) -> Row {
    vec![int(n), Value::Char(format!("text {n}"))]
}

/// The tables' files, in memory.
fn files() -> TableFiles {
    let fs = MemoryFileSystem::new();
    let pager = Arc::new(
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap(),
    );
    let pool = Arc::new(BufferPool::new(512, Box::new(Clock::new(512))).unwrap());
    let file = pool.add_pager(pager);
    TableFiles::new(pool, file, Arc::new(MemoryLog::new()))
}

/// `column = value`.
fn equal(column: usize, value: i128) -> Conjunction {
    Conjunction::of(Condition { columns: vec![column], probe: Probe::Compare { op: CompOp::Eq, value: int(value) } })
}

/// The first values of the rows `tx` reads of `table` where `conditions`,
/// judged of the rows the scan gives, as a filter judges them.
fn found(table: &Arc<RowTable>, tx: u64, conditions: &Conjunction) -> Vec<String> {
    let rows = Arc::clone(table).scan_where(tx, &[0], conditions).map(|r| r.unwrap().1);
    rows.filter(|row| conditions.matches(row)).map(|row| row[0].to_string()).collect()
}

/// Whether a scan told `conditions` reads the zone of row `id`.
fn reads(table: &RowTable, conditions: &Conjunction, id: RowId) -> bool {
    table.zones.ranges(Some(conditions)).iter().any(|&(first, end)| first <= id && end.is_none_or(|e| id < e))
}

/// Has `table` sum up its zones, of rows a reader at or after `horizon`
/// may read, until none is due.
fn summed(table: &RowTable, horizon: u64) {
    while table.maintain(horizon).unwrap() {}
}

/// Verifies: [DESIGN] RULES.md D31, D29, zone maps: a scan reads only the
/// zones whose rows its conditions may match, every zone for a column not
/// kept; a zone an update widened holds the old value while a reader of
/// it is open, and once summed up after, only the new; a deletion's zone
/// is narrowed alike; IS NULL reads no zone of no null.
#[test]
fn zones_skip_what_no_reader_can_match() {
    let files = files();
    let table = Arc::new(RowTable::create(&files, &TYPES, TableOptions::default()).unwrap());
    let ids: Vec<RowId> = (0..5000).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
    let far = ids[4999];
    assert!(far >= ZONE_ROWS.checked_mul(2).unwrap(), "rows of several zones");
    assert!(reads(&table, &equal(0, 10), ids[10]) && !reads(&table, &equal(0, 10), far));
    let text = Conjunction::of(Condition { columns: vec![1], probe: Probe::Compare { op: CompOp::Eq, value: Value::Char("x".into()) } });
    assert!(reads(&table, &text, far), "a column not kept");
    let null = Conjunction(vec![Conjunct::Null { column: 0, not: false }]);
    assert!(!reads(&table, &null, ids[10]), "no null");
    // The share of the rows a scan reads: those of the zone of 10, out of
    // the zones' rows; every row for a column not kept.
    let share = table.scan_share(&equal(0, 10));
    let in_zone = ids.iter().filter(|&&id| id.checked_div(ZONE_ROWS) == ids[10].checked_div(ZONE_ROWS)).count();
    let expected = f64::from(u32::try_from(in_zone).unwrap()) / 5000.0_f64;
    assert!((share - expected).abs() < 1e-9_f64, "{share} of {expected}");
    assert!((table.scan_share(&text) - 1.0_f64).abs() < 1e-9_f64);

    // The least value of the first zone, and the greatest of the next.
    let next_end = ids[0].checked_div(ZONE_ROWS).unwrap().checked_add(2).unwrap().checked_mul(ZONE_ROWS).unwrap();
    let last = ids.iter().rposition(|&id| id < next_end).unwrap();
    let greatest = i128::try_from(last).unwrap();
    let (reader, writer) = (20, 21);
    table.begin(reader, 1).unwrap();
    table.begin(writer, 1).unwrap();
    table.update(writer, ids[0], row(99_999)).unwrap();
    table.delete(writer, ids[last]).unwrap();
    table.validate(writer, 2).unwrap();
    table.commit(writer, 2);
    // The reader, of a snapshot before the update, holds the horizon.
    summed(&table, 1);
    assert!(table.maintenance_due(), "stale while the reader is open");
    assert_eq!(found(&table, reader, &equal(0, 0)), ["0"], "the old value, for the open reader");
    assert!(reads(&table, &equal(0, 99_999), ids[0]), "the new value too");
    table.commit(reader, 1);
    summed(&table, 3);
    assert!(!reads(&table, &equal(0, 0), ids[0]), "narrowed once no reader reads the old");
    assert!(!reads(&table, &equal(0, greatest), ids[last]), "the deleted row's value gone");
    assert_eq!(found(&table, NO_TRANSACTION, &equal(0, 99_999)), ["99999"]);
}

/// Verifies: [DESIGN] RULES.md D31, zone maps of a table opened: none is
/// known, so a scan reads every zone, until the maintenance thread has
/// summed each up; then it skips as before.
#[test]
fn an_opened_tables_zones_are_known_once_summed() {
    let files = files();
    let ids: Vec<RowId>;
    let meta = {
        let table = RowTable::create(&files, &TYPES, TableOptions::default()).unwrap();
        ids = (0..3000).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
        table.meta()
    };
    let table = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    assert!(table.maintenance_due());
    assert!(reads(&table, &equal(0, 10), ids[2999]), "not known: read");
    assert!((table.scan_share(&equal(0, 10)) - 1.0_f64).abs() < 1e-9_f64, "not known: every row");
    summed(&table, 1);
    assert!(!reads(&table, &equal(0, 10), ids[2999]), "known: skipped");
    assert_eq!(found(&table, NO_TRANSACTION, &equal(0, 2999)), ["2999"]);
}

/// Verifies: [DESIGN] RULES.md D31, zone maps summed up as rows are
/// written: a zone the maintenance thread sums up as a row is updated or
/// inserted into it, by a transaction open, either reads the row's new
/// version or is summed up again, so no scan told the row's value skips
/// the zone that holds it. Each round sets the thread summing up as it
/// updates a row and inserts another, as a statement's end wakes the
/// maintenance thread as the next begins, a little later each round.
#[test]
fn zones_summed_up_meanwhile_hold_every_row_written() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let files = files();
    let table = Arc::new(RowTable::create(&files, &TYPES, TableOptions::default()).unwrap());
    let first = table.insert(NO_TRANSACTION, row(0)).unwrap();
    // Rounds asked for, and done, by the thread summing up; 0 asks it to end.
    let (asked, done) = (Arc::new(AtomicU64::new(1)), Arc::new(AtomicU64::new(1)));
    let summing = {
        let (table, asked, done) = (Arc::clone(&table), Arc::clone(&asked), Arc::clone(&done));
        std::thread::spawn(move || {
            loop {
                let round = asked.load(Ordering::Acquire);
                if round == 0 {
                    return;
                }
                if round == done.load(Ordering::Acquire) {
                    std::hint::spin_loop();
                    continue;
                }
                // Begun a little later each round, so it begins at each
                // point of the insert in turn.
                for _ in 0..round.checked_rem(64).unwrap_or(0).saturating_mul(16) {
                    std::hint::spin_loop();
                }
                while table.sum_zones(u64::MAX).unwrap() {}
                done.store(round, Ordering::Release);
            }
        })
    };
    let written: Vec<i128> = (1_i128..2000_i128).map(|n| n.checked_mul(1_000).unwrap()).collect();
    let (tx, mut lost) = (10_u64, Vec::new());
    table.begin(tx, 1).unwrap();
    for (round, &v) in (2_u64..).zip(&written) {
        asked.store(round, Ordering::Release);
        table.update(tx, first, row(-v)).unwrap();
        table.insert(tx, row(v)).unwrap();
        while done.load(Ordering::Acquire) != round {
            std::hint::spin_loop();
        }
        let updated = v.checked_neg().unwrap();
        if found(&table, tx, &equal(0, updated)).is_empty() {
            lost.push(updated);
        }
    }
    asked.store(0, Ordering::Release);
    summing.join().unwrap();
    lost.extend(written.iter().copied().filter(|&v| found(&table, tx, &equal(0, v)).is_empty()));
    assert_eq!(lost, Vec::<i128>::new(), "rows a scan told their values skips");
}
