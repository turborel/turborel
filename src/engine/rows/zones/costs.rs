//! Unit tests of what zone maintenance costs (`zones/loose.rs`,
//! `zones/summed.rs`): a zone summed up in its loose columns alone, a hot
//! zone waiting longer each time, resting zones left alone unless asked
//! now, and bounds holding every row under writes, deletions and updates
//! summed up meanwhile.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::loose::{ALL, bit};
use super::summed::{HOT, MOST_WAIT, REST, nanos};
use super::tests::{TYPES, files, row, warmed};
use super::{Replaced, SAMPLED, ZONE_ROWS, Zone, Zones, zone_at};
use crate::engine::rows::{RowTable, TableOptions};
use crate::index::Probe;
use crate::storage::{Condition, Conjunction, Row, RowId, TableStorage};
use crate::sync::lock_builtin;
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, DataType, Decimal, Spelled, Value, compare};

/// Two numbers, both kept.
const PAIRS: [DataType; 2] = [
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer },
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer },
];

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn pair(a: i128, b: i128) -> Row {
    vec![int(a), int(b)]
}

/// A table of `PAIRS`, of a row `(n, n)` for each `n` of `values`: the ids.
fn pairs(values: std::ops::Range<i128>) -> (Arc<RowTable>, Vec<RowId>) {
    let table = Arc::new(warmed(RowTable::create(&files(), &PAIRS, TableOptions::default()).unwrap()));
    let ids = values.map(|n| table.insert(NO_TRANSACTION, pair(n, n)).unwrap()).collect();
    (table, ids)
}

/// Has `table` sum up every zone due, of rows any reader may read.
fn summed(table: &RowTable) {
    while table.maintain(u64::MAX, true).unwrap() {}
}

/// Has `edit` change the zone of row `id` of `table`, and gives back what
/// it gives.
fn with_zone<T>(table: &RowTable, id: RowId, edit: impl FnOnce(&mut Zone) -> T) -> T {
    edit(&mut lock_builtin(&table.zones.slots.get(zone_at(id)).unwrap().zone))
}

/// The bounds of column `place` of the zone of row `id`, as numbers.
fn bounds(table: &RowTable, id: RowId, place: usize) -> Option<(String, String)> {
    with_zone(table, id, |zone| zone.bounds[place].as_ref().map(|(l, g)| (l.to_string(), g.to_string())))
}

fn span(least: i128, greatest: i128) -> (String, String) {
    (int(least).to_string(), int(greatest).to_string())
}

/// Verifies: [DESIGN] RULES.md D770, zone maps: a write loosens only the
/// column whose bound it may leave unreached, and a sum up narrows that
/// column alone, keeping the other's bounds as they were, though wider
/// than the rows; a zone loose in every column, as one not known, is
/// summed up whole.
///
/// Regression test (R-TEST-7): a zone stale for one column was summed up
/// in every column kept, each version of its rows decoded whole, so a
/// table whose writes kept one column's bound moving paid for all.
#[test]
fn a_zone_is_summed_up_in_its_loose_columns_alone() {
    let (table, ids) = pairs(0..100);
    summed(&table);
    assert_eq!(with_zone(&table, ids[0], |zone| zone.loose), 0);
    // The second column's greatest shrunk: it alone loose.
    table.begin(10, 1).unwrap();
    table.update(10, ids[99], pair(99, 50)).unwrap();
    table.validate(10, 2).unwrap();
    table.commit(10, 2);
    assert_eq!(with_zone(&table, ids[0], |zone| zone.loose), bit(1), "the first column kept");
    // Wider than its rows, as a zone may be: left so if not loose.
    with_zone(&table, ids[0], |zone| zone.bounds[0] = Some((int(-1000), int(1000))));
    summed(&table);
    assert_eq!(bounds(&table, ids[0], 1), Some(span(0, 98)), "the loose column narrowed");
    assert_eq!(bounds(&table, ids[0], 0), Some(span(-1000, 1000)), "the other kept, not summed up");
    assert_eq!(with_zone(&table, ids[0], |zone| zone.loose), 0);
    with_zone(&table, ids[0], |zone| zone.loose = ALL);
    table.zones.stale.fetch_add(1, Ordering::AcqRel);
    summed(&table);
    assert_eq!(bounds(&table, ids[0], 0), Some(span(0, 99)), "every column summed up");
    assert_eq!(bounds(&table, ids[0], 1), Some(span(0, 98)));
}

/// Verifies: [DESIGN] RULES.md D771, zone maps: a zone looked at again
/// within `HOT` of its waits, `REST` at least, is hot, and waits twice as
/// long each time, from `REST` up to `MOST_WAIT`; one looked at after a
/// longer quiet waits not at all; a zone is ready once its wait is over.
/// MC/DC of the zone being hot: never looked at (not hot), looked at
/// within the window (hot) against past it (not hot).
#[test]
fn a_hot_zone_waits_twice_as_long_each_time() {
    let (rest, most) = (nanos(REST), nanos(MOST_WAIT));
    let mut zone = Zone::empty(true);
    assert!(zone.ready(0), "never looked at");
    zone.looked_at(0);
    assert_eq!(zone.wait, 0, "never looked at before: not hot");
    let mut clock = 0_u64;
    for wait in [1_u64, 2, 4, 8, 16, 32, 32] {
        clock = clock.saturating_add(zone.wait.max(rest));
        zone.looked_at(clock);
        assert_eq!(zone.wait, rest.saturating_mul(wait).min(most), "hot: doubled, to the most");
    }
    assert!(!zone.ready(clock.saturating_add(most).saturating_sub(1)) && zone.ready(clock.saturating_add(most)));
    zone.looked_at(clock.saturating_add(most.saturating_mul(HOT)));
    assert_eq!(zone.wait, 0, "looked at after a quiet: not hot");
}

/// Verifies: [DESIGN] RULES.md D771, zone maps: zones written without
/// pause are not summed up at every pass, but at passes further and
/// further apart as each waits twice as long, though due at each; the
/// maintenance thread asked to do every table's maintenance now sums them
/// all up.
#[test]
fn a_hot_table_is_not_summed_up_at_every_pass() {
    let zones = 4_usize;
    let table = Arc::new(warmed(RowTable::create(&files(), &TYPES, TableOptions::default()).unwrap()));
    let rows = u64::try_from(zones).unwrap().checked_mul(ZONE_ROWS).unwrap();
    let ids: Vec<RowId> = (0..rows).map(|n| table.insert(NO_TRANSACTION, row(i128::from(n))).unwrap()).collect();
    let mut firsts: Vec<RowId> = ids.clone();
    firsts.dedup_by_key(|id| zone_at(*id));
    summed(&table);
    let (start, rest) = (nanos(MOST_WAIT).saturating_mul(HOT), nanos(REST));
    let mut looks = 0_usize;
    for pass in 0..32_u64 {
        for &id in &firsts {
            table.update(NO_TRANSACTION, id, row(i128::from(id))).unwrap();
        }
        let clock = start.saturating_add(rest.saturating_mul(pass));
        table.sum_zones_at(u64::MAX, Some(clock)).unwrap();
        looks = looks.saturating_add(firsts.iter().filter(|&&id| with_zone(&table, id, |z| z.looked) == Some(clock)).count());
    }
    // Looked at the passes 0, 1, 2, 4, 8 and 16, not at each of 32.
    assert!(firsts.len() >= zones);
    assert_eq!(looks, firsts.len().checked_mul(6).unwrap());
    assert!(table.zones.due(), "due still");
    summed(&table);
    assert!(!table.zones.due(), "every zone summed up now");
}

/// Verifies: [DESIGN] RULES.md D772, zone maps: a table's maintenance
/// called while its zones rest, as for another table's work or an
/// index's, sums none up; called to do it now, it does.
///
/// Regression test (R-TEST-7): maintenance asked for anything else summed
/// up a table's zones as often as it was called, resting or not.
#[test]
fn resting_zones_are_summed_up_only_now() {
    let (table, ids) = pairs(0..10);
    summed(&table);
    table.begin(10, 1).unwrap();
    table.update(10, ids[9], pair(9, 5)).unwrap();
    table.validate(10, 2).unwrap();
    table.commit(10, 2);
    table.zones.rest_until.store(u64::MAX, Ordering::Release);
    assert!(!table.maintain(u64::MAX, false).unwrap(), "resting");
    assert!(table.zones.due());
    assert!(table.maintain(u64::MAX, true).unwrap(), "now");
    assert_eq!(bounds(&table, ids[0], 1), Some(span(0, 8)));
}

/// The row `n` of the `writer`'s rows written in round `round`: values
/// moving out and in, on bounds and inside them.
fn written(writer: i128, round: i128, n: i128) -> Row {
    let a = (n.wrapping_mul(7919).wrapping_add(round.wrapping_mul(31))).rem_euclid(5000);
    pair(a.wrapping_add(writer.wrapping_mul(10_000)), a.wrapping_neg())
}

/// Has `writer` insert rows of its own into `table`, then over rounds
/// update each, or delete it and insert another in its place.
fn write(table: &RowTable, writer: i128) {
    let mut ids: Vec<RowId> = (0..1500).map(|n| table.insert(NO_TRANSACTION, written(writer, 0, n)).unwrap()).collect();
    for round in 1..6_i128 {
        for (n, id) in (0_i128..).zip(ids.iter_mut()) {
            let row = written(writer, round, n);
            if n.rem_euclid(5) == round.rem_euclid(5) {
                table.delete(NO_TRANSACTION, *id).unwrap();
                *id = table.insert(NO_TRANSACTION, row).unwrap();
            } else {
                table.update(NO_TRANSACTION, *id, row).unwrap();
            }
        }
    }
}

/// Whether `v`, of column `place` of row `id`, lies within its zone.
fn within(table: &RowTable, (id, place): (RowId, usize), v: &Value) -> bool {
    let le = |a: &Value, b: &Value| compare(a, b).is_some_and(std::cmp::Ordering::is_le);
    bounds_of(table, id, place).is_some_and(|(l, g)| le(&l, v) && le(v, &g))
}

/// Asserts each zone of `table` holding a row of `rows` is as narrow as
/// they are, in each column.
fn assert_narrow(table: &RowTable, rows: &[(RowId, Row)]) {
    let mut zones: BTreeMap<usize, (RowId, Vec<&Row>)> = BTreeMap::new();
    for (id, row) in rows {
        zones.entry(zone_at(*id)).or_insert_with(|| (*id, Vec::new())).1.push(row);
    }
    for (id, held) in zones.values() {
        for place in 0..2 {
            let least = held.iter().map(|r| &r[place]).min_by(|a, b| compare(a, b).unwrap()).unwrap();
            let greatest = held.iter().map(|r| &r[place]).max_by(|a, b| compare(a, b).unwrap()).unwrap();
            let narrow = Some((least.to_string(), greatest.to_string()));
            assert_eq!(bounds_of(table, *id, place).map(|(l, g)| (l.to_string(), g.to_string())), narrow, "as narrow as its rows");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D31, D770, zone maps under writes summed up
/// meanwhile: two writers update, delete and insert rows of their own,
/// moving values past bounds and back inside, as a third thread sums the
/// zones up without pause; every row is then held by its zone, in each
/// column, and once summed up with no writer left each zone is as narrow
/// as its rows.
#[test]
fn zones_hold_every_row_under_writes_summed_up_meanwhile() {
    let (table, _) = pairs(0..0);
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Acquire) {
                table.sum_zones(u64::MAX, true).unwrap();
            }
        });
        let shared = &table;
        let writers: Vec<_> = (0..2_i128).map(|writer| scope.spawn(move || write(shared, writer))).collect();
        for writer in writers {
            writer.join().unwrap();
        }
        stop.store(true, Ordering::Release);
    });
    let rows: Vec<(RowId, Row)> = Arc::clone(&table).scan(NO_TRANSACTION, &[0, 1]).map(Result::unwrap).collect();
    assert_eq!(rows.len(), 3000);
    let held = |(id, row): &(RowId, Row)| within(&table, (*id, 0), &row[0]) && within(&table, (*id, 1), &row[1]);
    assert!(rows.iter().all(held), "a row outside its zone");
    summed(&table);
    assert_narrow(&table, &rows);
}

/// The bounds of column `place` of the zone of row `id`.
fn bounds_of(table: &RowTable, id: RowId, place: usize) -> Option<(Value, Value)> {
    with_zone(table, id, |zone| zone.bounds[place].clone())
}

/// Zones of one row each, the row of zone `k` holding `k`, `count` of them.
fn one_a_zone(count: usize) -> Zones {
    let zones = Zones::new(&TYPES, true, None);
    zones.begin_warm();
    for k in 0..count {
        let n = i128::try_from(k).unwrap();
        zones.widen(u64::try_from(k).unwrap().checked_mul(ZONE_ROWS).unwrap(), Some(&row(n)), Replaced::Nothing);
    }
    zones
}

/// The share of the rows a scan for the first column equal to `n` reads.
fn share_of(zones: &Zones, n: i128) -> f64 {
    let probe = Probe::Compare { op: CompOp::Eq, value: Value::Exact(Decimal::new(n, 0)) };
    zones.share(&Conjunction::of(Condition { columns: vec![0], probe }))
}

/// Verifies: [DESIGN] RULES.md D838, D839: a scan's share of a sample of
/// the zones is never less than one zone of the sample, as the zones
/// passed over may hold the rows asked for, of a value in a zone the
/// sample passes over, as of one in a zone it reads. Of every zone looked
/// at, a value no zone holds now is still read in one, as rows written
/// later may hold it.
#[test]
fn a_sampled_share_is_never_less_than_one_zone_of_it() {
    let zones = one_a_zone(SAMPLED + 44);
    let sampled = f64::from(u32::try_from((SAMPLED + 44).div_ceil(2)).unwrap());
    assert!((share_of(&zones, 1) - 1.0_f64 / sampled).abs() < 1e-9_f64, "passed over: {}", share_of(&zones, 1));
    assert!((share_of(&zones, 0) - 1.0_f64 / sampled).abs() < 1e-9_f64, "read: {}", share_of(&zones, 0));
    let every = one_a_zone(SAMPLED);
    let one = 1.0_f64 / f64::from(u32::try_from(SAMPLED).unwrap());
    assert!((share_of(&every, -5) - one).abs() < 1e-9_f64, "every zone looked at, none holding it");
    assert!((share_of(&every, 7) - one).abs() < 1e-9_f64);
}
