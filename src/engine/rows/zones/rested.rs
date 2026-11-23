//! Unit tests of the zones' rest (`zones/summed.rs`): after a pass over
//! every zone due, and after one over every zone, more due again.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::tests::{TYPES, files, row, warmed};
use super::{MOST_SUMMED, ZONE_ROWS};
use crate::engine::rows::{RowTable, TableOptions};
use crate::storage::{RowId, TableStorage};
use crate::txn::NO_TRANSACTION;

/// A table of `zones` zones, each full of rows: the ids of its rows.
fn table_of(zones: usize) -> (Arc<RowTable>, Vec<RowId>) {
    let table = Arc::new(warmed(RowTable::create(&files(), &TYPES, TableOptions::default()).unwrap()));
    let rows = u64::try_from(zones).unwrap().checked_mul(ZONE_ROWS).unwrap();
    let ids = (0..rows).map(|n| table.insert(NO_TRANSACTION, row(i128::from(n))).unwrap()).collect();
    (table, ids)
}

/// Verifies: [DESIGN] RULES.md D31, zone maps: once a pass of the
/// maintenance thread has looked at every zone due, the zones rest, so a
/// write meanwhile leaves work due but asks for no maintenance until the
/// rest is over. MC/DC of the pass's end, every zone not yet looked at
/// since the last rest: a pass looking at fewer zones than one call sums
/// up rests them; one of as many, more due, does not.
#[test]
fn zones_rest_after_a_pass_over_every_zone_due() {
    let (table, ids) = table_of(MOST_SUMMED.checked_add(1).unwrap());
    assert!(table.sum_zones(u64::MAX, false).unwrap());
    assert!(table.zones.wanted(), "more zones due than one call sums up: no rest");
    assert!(table.sum_zones(u64::MAX, false).unwrap());
    assert!(!table.zones.due() && !table.zones.wanted(), "every zone looked at");
    table.update(NO_TRANSACTION, ids[0], row(-1)).unwrap();
    assert!(table.zones.due() && !table.zones.wanted(), "due, but resting");
    table.zones.rest_until.store(0, Ordering::Release);
    assert!(table.zones.wanted(), "the rest over");
}

/// Verifies: [DESIGN] RULES.md D31, zone maps: zones writes make due again
/// as fast as they are summed up rest once every zone has been looked at
/// since the last rest, though a call finds as many due as it sums up, so
/// the maintenance thread sums each up once a rest, not over and over.
/// MC/DC of the pass's end, as many due as one call sums up: every zone
/// looked at rests them; not yet, they do not.
#[test]
fn zones_kept_due_rest_after_a_pass_over_every_zone() {
    let zones = MOST_SUMMED.checked_add(1).unwrap();
    let (table, ids) = table_of(zones);
    assert!(table.sum_zones(u64::MAX, false).unwrap());
    assert!(table.zones.wanted(), "one zone not looked at yet: no rest");
    // Each zone the first call summed up made due again by a write.
    let zone_ids = usize::try_from(ZONE_ROWS).unwrap();
    for zone in 0..MOST_SUMMED {
        table.update(NO_TRANSACTION, ids[zone.checked_mul(zone_ids).unwrap()], row(-1)).unwrap();
    }
    assert!(table.sum_zones(u64::MAX, false).is_ok());
    assert!(table.zones.due(), "the zones summed last are due still");
    assert!(!table.zones.wanted(), "every one of the {zones} zones looked at: resting");
}
