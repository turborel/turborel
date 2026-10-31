//! Unit tests of a row store table's zone maps (`zones.rs`).

use std::sync::Arc;

use super::ZONE_ROWS;
use super::saved::SEGMENT_ZONES;
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
    paged().1
}

/// The tables' files, in memory, and their pager.
fn paged() -> (Arc<Pager>, TableFiles) {
    let fs = MemoryFileSystem::new();
    let pager = Arc::new(
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap(),
    );
    let pool = Arc::new(BufferPool::new(512, Box::new(Clock::new(512))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    (pager, TableFiles::new(pool, file, Arc::new(MemoryLog::new())))
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

/// Verifies: [DESIGN] RULES.md D31, zone maps of a table opened whose
/// zones no checkpoint saved: none is known, so a scan reads every zone,
/// until the maintenance thread has summed each up; then it skips as
/// before.
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

/// Verifies: [DESIGN] RULES.md D31, zone maps of versions put back: a row a
/// transaction inserted, then deleted, has no version left in its zone
/// once summed up; the deletion undone, to a savepoint or as a statement
/// fails, puts the row back and widens its zone to hold it, so a scan
/// told its value finds it. Without the widening, the zone stayed summed
/// up without the row (constraints::partial_matches_11_8, of a MATCH
/// PARTIAL foreign key whose DELETE deleted nothing).
#[test]
fn a_version_put_back_is_held_by_its_zone() {
    let files = files();
    let table = Arc::new(RowTable::create(&files, &TYPES, TableOptions::default()).unwrap());
    let tx = 10_u64;
    table.begin(tx, 1).unwrap();
    let id = table.insert(tx, row(77)).unwrap();
    let before = table.savepoint(tx);
    table.delete(tx, id).unwrap();
    summed(&table, u64::MAX);
    assert!(found(&table, tx, &equal(0, 77)).is_empty(), "deleted");
    table.rollback_to(tx, before).unwrap();
    assert_eq!(found(&table, tx, &equal(0, 77)), vec!["77"], "put back, and held by its zone");
}

/// Verifies: [DESIGN] RULES.md D31, zone maps of an insert undone: the row
/// taken out, its zone is stale, and once summed up no longer holds the
/// row's value, so a scan told it skips the zone again; as a savepoint's
/// insert is undone too.
///
/// Regression test (R-TEST-7): an insert undone left its zone widened to
/// hold the row taken out, never marked stale, so the maintenance thread
/// never narrowed it, and every scan told the value read the zone.
#[test]
fn an_undone_inserts_zone_is_narrowed() {
    let table = Arc::new(RowTable::create(&files(), &TYPES, TableOptions::default()).unwrap());
    let first = table.insert(NO_TRANSACTION, row(0)).unwrap();
    let tx = 10_u64;
    table.begin(tx, 1).unwrap();
    let saved = table.savepoint(tx);
    let kept = table.insert(tx, row(500)).unwrap();
    table.rollback_to(tx, saved).unwrap();
    let undone = table.insert(tx, row(1_000_000)).unwrap();
    assert!(reads(&table, &equal(0, 1_000_000), undone), "widened by the insert");
    table.rollback(tx).unwrap();
    assert!(table.maintenance_due(), "stale once undone");
    summed(&table, u64::MAX);
    assert!(!table.maintenance_due());
    for (value, id) in [(1_000_000, undone), (500, kept)] {
        assert!(!reads(&table, &equal(0, value), id), "{value} no longer held");
    }
    assert!(reads(&table, &equal(0, 0), first), "the row kept still held");
    assert_eq!(found(&table, NO_TRANSACTION, &equal(0, 1_000_000)), Vec::<String>::new());
}

/// Verifies: [DESIGN] RULES.md D31, D25, zone maps kept with the table's
/// pages: saved, as a checkpoint saves them, a table opened knows its
/// zones with no sum up due, so a scan skips at once; a zone stale when
/// saved is stale once opened, and narrowed once summed up; a row
/// recovery writes widens its zone (`apply`); a second save writes the
/// changed zones again. Zones of row ids past those saved, reserved but
/// not saved, are not known, so read, until summed up.
#[test]
fn a_tables_zones_saved_are_known_once_opened() {
    let files = files();
    let (ids, meta) = {
        let table = RowTable::create(&files, &TYPES, TableOptions::default()).unwrap();
        let ids: Vec<RowId> = (0..3000).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
        table.update(NO_TRANSACTION, ids[0], row(500)).unwrap();
        files.save_zones().unwrap();
        (ids, table.meta())
    };
    let table = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    assert!(!reads(&table, &equal(0, 10), ids[2999]), "known as saved: skipped, no sum up made");
    assert!(reads(&table, &equal(0, 0), ids[0]), "stale as saved");
    assert!(table.maintenance_due(), "the stale zone");
    summed(&table, u64::MAX);
    assert!(!reads(&table, &equal(0, 0), ids[0]) && reads(&table, &equal(0, 500), ids[0]), "narrowed once summed up");
    let recovered = ids[2999].checked_add(1).unwrap();
    table.apply(recovered, Some(row(7_000_000))).unwrap();
    assert_eq!(found(&table, NO_TRANSACTION, &equal(0, 7_000_000)), ["7000000"], "widened by recovery");
    files.save_zones().unwrap();
    drop(table);
    let again = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    assert!(!reads(&again, &equal(0, 0), ids[0]) && !reads(&again, &equal(0, 10), ids[2999]), "narrow as saved");
    assert_eq!(found(&again, NO_TRANSACTION, &equal(0, 7_000_000)), ["7000000"]);
    // Rows of ids past the zones saved, of no save.
    let past: Vec<RowId> = (0..3000).map(|n| again.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
    drop(again);
    let opened = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    assert!(opened.maintenance_due() && reads(&opened, &equal(0, -1), past[2999]), "not known: read");
    summed(&opened, u64::MAX);
    assert!(!opened.maintenance_due() && !reads(&opened, &equal(0, -1), past[2999]), "known once summed up");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.18 GR2], RULES.md D31,
/// D33: a dropped table frees the pages its zone maps were saved in with
/// the rest, so every page it took is free once it goes.
#[test]
fn a_dropped_tables_saved_zones_are_freed() {
    let (pager, files) = paged();
    let in_use = || pager.page_count() - pager.free_pages();
    let before = in_use();
    let table = RowTable::create(&files, &TYPES, TableOptions::default()).unwrap();
    for n in 0..5000 {
        table.insert(NO_TRANSACTION, row(n)).unwrap();
    }
    files.save_zones().unwrap();
    assert!(in_use() > before);
    table.discard();
    drop(table);
    assert_eq!(in_use(), before, "every page freed, the zones' among them");
    files.save_zones().unwrap();
}

/// Verifies: [DESIGN] RULES.md D31, D48, zone maps saved of a table whose
/// encodings cannot seal them, not registered: each segment is saved
/// empty, so the table opened knows none of its zones and sums them up;
/// a segment malformed, of other columns than the table keeps or sealed
/// as no encoding reads, is refused as the table opens (SQLCODE -916).
#[test]
fn zones_unsealable_or_malformed_are_not_known() {
    let files = files();
    let table = RowTable::create(&files, &TYPES, TableOptions::default()).unwrap();
    for n in 0..2000 {
        table.insert(NO_TRANSACTION, row(n)).unwrap();
    }
    let meta = table.meta();
    table.zones.seal_with(&Arc::new(crate::engine::rows::sealing::Sealing::named(Arc::default(), "nothing registered")));
    files.save_zones().unwrap();
    let segment = table.zones.place.lock().unwrap().as_ref().map(|p| p.segments[0]).unwrap();
    drop(table);
    let opened = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    assert!(opened.maintenance_due() && reads(&opened, &equal(0, -1), 0), "not known");
    summed(&opened, u64::MAX);
    assert!(!reads(&opened, &equal(0, -1), 0), "summed up");
    drop(opened);
    // A segment sealed by no encoding, of 5 columns kept; then one naming
    // more encodings than any pipeline holds.
    for stored in [vec![0_u8, 5, 0, 0, 0, 0, 0, 0, 0], vec![255_u8]] {
        *files.pool.write(files.file, segment).unwrap() = stored;
        let refused = RowTable::open(&files, meta, &TYPES, TableOptions::default()).err().map(|e| e.code);
        assert_eq!(refused, Some(crate::error::Code::Corrupt));
    }
}

/// Verifies: [DESIGN] RULES.md D31, D30, D38, zone maps saved, a failure:
/// a save, as a checkpoint makes it, whose pool write fails, every frame
/// held (SQLCODE -918), fails, and marks every segment to be saved again,
/// that of the table's first rows and that of a row recovery wrote far
/// past them; the next save writes them all, and the table opened knows
/// the zones of both as saved, with no sum up.
#[test]
fn zones_a_failed_save_left_are_saved_by_the_next() {
    let fs = MemoryFileSystem::new();
    let pager =
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let frames = 8_usize;
    let pool = Arc::new(BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(Arc::clone(&pool), file, Arc::new(MemoryLog::new()));
    let others: Vec<u64> = (0..frames).map(|_| pool.new_page(file).unwrap().0.0).collect();
    let table = RowTable::create(&files, &TYPES, TableOptions::default()).unwrap();
    let first = table.insert(NO_TRANSACTION, row(1)).unwrap();
    let segment_rows = u64::try_from(SEGMENT_ZONES).unwrap().checked_mul(ZONE_ROWS).unwrap();
    let far = first.checked_add(segment_rows).unwrap();
    table.apply(far, Some(row(2))).unwrap();
    let meta = table.meta();
    {
        let held: Vec<_> = others.iter().map(|&page| pool.read(file, page).unwrap()).collect();
        assert_eq!(held.len(), frames, "every frame held");
        assert_eq!(files.save_zones().err().map(|e| e.code), Some(crate::error::Code::Resources));
    }
    files.save_zones().unwrap();
    drop(table);
    let opened = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    assert!(!reads(&opened, &equal(0, -1), first) && !reads(&opened, &equal(0, -1), far), "both segments saved");
    assert_eq!(found(&opened, NO_TRANSACTION, &equal(0, 2)), ["2"]);
}
