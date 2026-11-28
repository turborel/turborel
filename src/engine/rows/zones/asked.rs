//! Unit tests of a table opened whose zones wait until one is asked for
//! (`zones/cold.rs`, `zones/summed.rs`, RULES.md D1212).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::tests::{TYPES, equal, files, reads, row, warmed};
use super::{MOST_SUMMED, ZONE_ROWS};
use crate::disk::pager::PageId;
use crate::engine::rows::{RowTable, TableFiles, TableOptions};
use crate::memory::MemoryBudget;
use crate::spill::WorkAreas;
use crate::storage::{RowId, TableStorage};
use crate::txn::NO_TRANSACTION;

/// A table of `zones` zones, each full of rows, written cold and warmed
/// after, so none of its zones is known, saved so and closed. Gives its
/// meta page and its rows' ids.
fn saved_unknown(files: &TableFiles, zones: usize) -> (PageId, Vec<RowId>) {
    let table = RowTable::create(files, &TYPES, TableOptions::default()).unwrap();
    let rows = u64::try_from(zones).unwrap().checked_mul(ZONE_ROWS).unwrap();
    let ids = (0..rows).map(|n| table.insert(NO_TRANSACTION, row(i128::from(n))).unwrap()).collect();
    table.zones.warm();
    files.save_zones().unwrap();
    (table.meta(), ids)
}

/// The zones of `table` stale or not known.
fn stale(table: &RowTable) -> usize {
    table.zones.stale.load(Ordering::Acquire)
}

/// Verifies: [DESIGN] RULES.md D1212: a table opened whose zones are not
/// known, none asked for, has no maintenance due, and a call of the
/// maintenance thread sums up none of them; the first scan told a
/// condition of a column kept asks, and they are due and summed up. MC/DC
/// of the zones wanted (R-TEST-8): zones due, none asked for, and one
/// asked for. A regression test (R-TEST-7): it fails if a table opened is
/// summed up before a zone of it is asked for.
#[test]
fn an_opened_tables_zones_wait_until_one_is_asked_for() {
    let files = files();
    let (meta, ids) = saved_unknown(&files, 3);
    let table = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    assert!(!table.zones.cold() && !table.zones.asked(), "warm as saved, none asked for");
    assert!(table.zones.due() && !table.zones.wanted() && !table.maintenance_due());
    let unknown = stale(&table);
    assert!(unknown >= 3, "every zone of its rows");
    assert!(!table.maintain(u64::MAX, false).unwrap(), "the thread's call does no work");
    assert_eq!(stale(&table), unknown, "none summed up");
    assert!(reads(&table, &equal(0, 5), ids[2999]), "asked: not known, so read");
    assert!(table.zones.asked() && table.zones.wanted() && table.maintenance_due());
    assert!(table.maintain(u64::MAX, false).unwrap(), "the thread's call sums them up");
    assert_eq!(stale(&table), 0);
    assert!(!reads(&table, &equal(0, 5), ids[2999]), "known: skipped");
}

/// Verifies: [DESIGN] RULES.md D1212: each of what asks a zone makes the
/// zones of a table opened wanted: a read asking whether a row is visible
/// to every reader, a count of the rows settled, and a plan weighing an
/// index-only read; a scan told a condition of a column not kept, and the
/// planner's question of the share a scan reads, ask none.
#[test]
fn each_ask_of_a_zone_makes_an_opened_tables_wanted() {
    let files = files();
    let (meta, ids) = saved_unknown(&files, 1);
    let open = || Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    let table = open();
    assert_eq!(table.zones.ranges(Some(&equal(1, 5))), [(0, None)], "a column not kept");
    assert!(table.zones.ranges(None) == [(0, None)] && table.zones.share(&equal(0, 5)) > 0.0_f64);
    assert!(!table.zones.asked() && !table.maintenance_due(), "none of these asks");
    assert!(!table.visible_to_all(ids[0]) && table.maintenance_due(), "whether a row is visible");
    let counted = open();
    assert_eq!(counted.zones.settled().0, 0);
    assert!(counted.maintenance_due(), "a count");
    let weighed = open();
    assert!(weighed.settled_share() == Some(1.0_f64) && weighed.maintenance_due(), "an index-only read weighed");
}

/// Verifies: [DESIGN] RULES.md D1212, D31: the zones summed up before a
/// table is closed are saved, so opened again it has only the others to
/// sum up, once one is asked for; and a table made warm in this session
/// has its zones wanted as it is written, none asked for.
#[test]
fn a_sum_up_saved_is_resumed_once_asked_again() {
    let files = files();
    let zones = MOST_SUMMED.checked_add(2).unwrap();
    let (meta, ids) = saved_unknown(&files, zones);
    let left = {
        let table = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
        let unknown = stale(&table);
        table.zones.warm();
        assert!(table.sum_zones_at(u64::MAX, Some(0)).unwrap(), "one call's zones");
        assert_eq!(stale(&table), unknown.checked_sub(MOST_SUMMED).unwrap());
        files.save_zones().unwrap();
        stale(&table)
    };
    assert!(left >= 2, "more zones than one call sums up");
    let table = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    assert_eq!(stale(&table), left, "the zones summed up are known as saved");
    assert!(!table.maintenance_due() && !table.maintain(u64::MAX, false).unwrap(), "none asked for yet");
    assert!(!reads(&table, &equal(0, 5), ids[2000]), "a zone summed up before the close: skipped");
    assert!(table.maintenance_due() && table.maintain(u64::MAX, false).unwrap(), "asked: the rest summed up");
    assert_eq!(stale(&table), 0);
    let made = warmed(RowTable::create(&files, &TYPES, TableOptions::default()).unwrap());
    made.insert(NO_TRANSACTION, row(1)).unwrap();
    assert!(made.zones.asked() && made.maintenance_due(), "made warm: wanted as written");
}

/// Verifies: [DESIGN] RULES.md D1212: the first ask of the zones of a
/// table opened is told to the work areas its store is attached to,
/// once: a table opened none of whose zones was asked for tells nothing,
/// the first ask leaves a note, and an ask after it none; a table made
/// in this session tells nothing as it is warmed. A regression test
/// (R-TEST-7): it fails if a read of a table made in the session asks
/// for the maintenance job, whose pass then changed the plans made
/// beside it from one run to the next.
#[test]
fn the_first_ask_is_told_to_the_work_areas_once() {
    let files = files();
    let work = Arc::new(WorkAreas::new(MemoryBudget::new(1 << 20)));
    *files.work.write().unwrap() = Some(Arc::clone(&work));
    let (meta, ids) = saved_unknown(&files, 1);
    assert!(!work.maintenance().take(), "a table made in this session tells nothing");
    let table = Arc::new(RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap());
    assert!(!work.maintenance().take(), "none asked for");
    assert!(reads(&table, &equal(0, 5), ids[0]));
    assert!(work.maintenance().take() && table.maintenance_due(), "the first ask: told");
    assert!(reads(&table, &equal(0, 5), ids[0]) && !work.maintenance().take(), "told once");
}
