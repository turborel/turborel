//! \[DESIGN\] A row store table's zone maps (RULES.md D31): for each block
//! of `ZONE_ROWS` row ids, of each column kept, the least and greatest
//! value not null its rows hold, and whether one is null; so a scan told
//! conditions (`TableStorage::scan_where`) reads only the blocks whose rows
//! they may match. Row ids never move, so a zone is not touched as leaves
//! split or are taken out.
//!
//! A zone covers every version of its rows a snapshot may read: a write
//! widens it before it is made, and nothing else changes it but the
//! database's maintenance thread (`TableStorage::maintain`). That narrows a
//! zone an update or a deletion left wider than its rows, summing it up
//! again from every version of its rows a reader at or after the horizon
//! may read, an open transaction's among them; a write meanwhile counts a
//! change of the zone, and its sum is then thrown away. A zone is never
//! narrower than its rows, at most wider.
//!
//! Kept in memory: a table opened knows none of its zones, reading every
//! block, until the maintenance thread has summed each up. Kept are
//! numbers, datetimes, intervals, and character strings of at most
//! `SHORT` characters, so a zone takes a few bytes of each row.

use std::cmp::Ordering;
use std::ops::Bound;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Mutex, RwLock};

use super::RowTable;
use super::version::{Maker, Version};
use crate::engine::rows::{key, row_id};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::{Conjunct, Conjunction, Row, RowId};
use crate::sync::{lock_builtin, read, write};
use crate::txn::Timestamp;
use crate::value::{DataType, Value, compare};

/// \[DESIGN\] The row ids of a zone: a thread's block of them is four
/// (`rows/ids.rs`).
pub(super) const ZONE_ROWS: u64 = 1024;
/// \[DESIGN\] The longest character string a zone keeps the bounds of.
const SHORT: u32 = 32;
/// \[DESIGN\] The most zones one call of the maintenance thread sums up.
const MOST_SUMMED: usize = 64;

/// A zone: of each column kept, its least and greatest value not null and
/// whether one is null.
#[derive(Debug, Clone, Default)]
struct Zone {
    bounds: Vec<Option<(Value, Value)>>,
    nulls: Vec<bool>,
    /// Its changes: a sum made over one is thrown away.
    changes: u64,
    /// Whether it may be wider than its rows, or is not known at all.
    stale: bool,
    known: bool,
}

/// The zones of a table, by the first row id of each over `ZONE_ROWS`,
/// each locked alone, so writers of different blocks of row ids, as
/// threads are given (`rows/ids.rs`), do not wait for one another.
#[derive(Default)]
struct Map {
    zones: Vec<Mutex<Zone>>,
    /// Zones made for row ids below this are not known until summed up.
    unknown_below: RowId,
}

/// \[DESIGN\] A table's zone maps.
pub(super) struct Zones {
    /// The columns kept, by their places in a row.
    kept: Vec<usize>,
    /// Locked to be written only to add zones.
    map: RwLock<Map>,
    /// The zones stale or not known.
    stale: AtomicUsize,
    /// The zone a sum up looks from, so zones a reader keeps stale do not
    /// keep the others waiting.
    next_sum: AtomicUsize,
}

/// Whether a column of type `t` is kept.
fn kept(t: &DataType) -> bool {
    match t {
        DataType::Character { length } | DataType::Varying { length } => *length <= SHORT,
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_) => true,
        DataType::Bit { .. } | DataType::BitVarying { .. } | DataType::Extension(_) => false,
    }
}

/// `bounds` widened to hold `v`.
fn widened(bounds: &mut Option<(Value, Value)>, v: &Value) {
    match bounds {
        None => *bounds = Some((v.clone(), v.clone())),
        Some((least, greatest)) => {
            if compare(v, least) == Some(Ordering::Less) {
                *least = v.clone();
            }
            if compare(v, greatest) == Some(Ordering::Greater) {
                *greatest = v.clone();
            }
        }
    }
}

impl Zone {
    /// A zone of no row, known or not, of `columns` kept.
    fn empty(columns: usize, known: bool) -> Zone {
        Zone { bounds: vec![None; columns], nulls: vec![false; columns], changes: 0, stale: !known, known }
    }

    /// Widens it to hold `values`, those of the columns kept.
    fn hold<'v>(&mut self, values: impl Iterator<Item = &'v Value>) {
        for ((bounds, null), v) in self.bounds.iter_mut().zip(self.nulls.iter_mut()).zip(values) {
            if v.is_null() {
                *null = true;
            } else {
                widened(bounds, v);
            }
        }
    }
}

impl Zones {
    /// The zone maps of a table of columns of `types`, of no row yet; none
    /// kept unless `keeping`.
    pub(super) fn new(types: &[DataType], keeping: bool) -> Zones {
        let kept_columns = if keeping { types.iter().enumerate().filter(|(_, t)| kept(t)).map(|(c, _)| c).collect() } else { Vec::new() };
        Zones { kept: kept_columns, map: RwLock::new(Map::default()), stale: AtomicUsize::new(0), next_sum: AtomicUsize::new(0) }
    }

    /// Of a table opened, whose rows' ids are below `below`: their zones
    /// not known until summed up.
    pub(super) fn opened(&self, below: RowId) {
        if self.kept.is_empty() {
            return;
        }
        let Ok(mut map) = write(&self.map) else { return };
        map.unknown_below = below;
        let count = usize::try_from(below.div_ceil(ZONE_ROWS)).unwrap_or(usize::MAX);
        let columns = self.kept.len();
        map.zones = (0..count).map(|_| Mutex::new(Zone::empty(columns, false))).collect();
        self.stale.store(count, AtomicOrdering::Release);
    }

    /// Widens the zone of row `id` to hold `row`, before it is written;
    /// marked stale if the row replaces another, which it may then be
    /// wider than.
    pub(super) fn widen(&self, id: RowId, row: Option<&Row>, replaces: bool) {
        if self.kept.is_empty() {
            return;
        }
        let at = usize::try_from(id.checked_div(ZONE_ROWS).unwrap_or(0)).unwrap_or(usize::MAX);
        if let Ok(map) = read(&self.map)
            && let Some(zone) = map.zones.get(at)
        {
            self.change(&mut lock_builtin(zone), row, replaces);
            return;
        }
        let Ok(mut map) = write(&self.map) else { return };
        while map.zones.len() <= at {
            let first = u64::try_from(map.zones.len()).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
            let known = first >= map.unknown_below;
            if !known {
                self.stale.fetch_add(1, AtomicOrdering::AcqRel);
            }
            let columns = self.kept.len();
            map.zones.push(Mutex::new(Zone::empty(columns, known)));
        }
        if let Some(zone) = map.zones.get(at) {
            self.change(&mut lock_builtin(zone), row, replaces);
        }
    }

    /// `widen`'s change of `zone`.
    fn change(&self, zone: &mut Zone, row: Option<&Row>, replaces: bool) {
        zone.changes = zone.changes.saturating_add(1);
        if let Some(row) = row {
            zone.hold(self.kept.iter().map(|&c| row.get(c).unwrap_or(&Value::Null)));
        }
        if replaces && !zone.stale {
            zone.stale = true;
            self.stale.fetch_add(1, AtomicOrdering::AcqRel);
        }
    }

    /// The ranges of row ids a scan told `conditions` reads, in order, each
    /// from its first id up to, not including, its end, if it has one.
    pub(super) fn ranges(&self, conditions: Option<&Conjunction>) -> Vec<(RowId, Option<RowId>)> {
        let all = vec![(0, None)];
        let (Some(conditions), false) = (conditions, self.kept.is_empty()) else { return all };
        let Ok(map) = read(&self.map) else { return all };
        let mut ranges: Vec<(RowId, Option<RowId>)> = Vec::new();
        for (at, zone) in map.zones.iter().enumerate() {
            if !self.may_match(&lock_builtin(zone), conditions) {
                continue;
            }
            let first = u64::try_from(at).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
            let end = first.saturating_add(ZONE_ROWS);
            // A range that goes on from the one before joins it.
            if let Some((_, Some(last))) = ranges.last_mut().filter(|r| r.1 == Some(first)) {
                *last = end;
            } else {
                ranges.push((first, Some(end)));
            }
        }
        // Rows past the last zone, written as the scan begins.
        let past = u64::try_from(map.zones.len()).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
        ranges.push((past, None));
        ranges
    }

    /// Whether a row of `zone` may be one each of `conditions` matches:
    /// so if the zone is not known, or a condition reads a column not kept.
    fn may_match(&self, zone: &Zone, conditions: &Conjunction) -> bool {
        zone_may_match(zone, conditions, &self.kept)
    }

    /// Whether a zone is stale or not known.
    pub(super) fn due(&self) -> bool {
        self.stale.load(AtomicOrdering::Acquire) > 0
    }
}

/// Whether a row of `zone`, of the columns `kept`, may be one each of
/// `conditions` matches (`Zones::may_match`).
fn zone_may_match(zone: &Zone, conditions: &Conjunction, kept: &[usize]) -> bool {
    if !zone.known {
        return true;
    }
    conditions.conjuncts().iter().all(|c| {
        let places: Option<Vec<usize>> = c.columns().iter().map(|col| kept.iter().position(|k| k == col)).collect();
        let Some(places) = places else { return true };
        match (c, places.as_slice()) {
            (Conjunct::Null { not: false, .. }, [at]) => zone.nulls.get(*at).copied().unwrap_or(true),
            (Conjunct::Null { not: true, .. }, [at]) => zone.bounds.get(*at).is_none_or(Option::is_some),
            (Conjunct::Condition(_) | Conjunct::In { .. } | Conjunct::Null { .. }, _) => {
                c.may_match(&bounds_of(zone, &c.columns(), &places))
            }
        }
    })
}

/// The bounds of `zone` of `columns`, kept at `places`, as a row's
/// columns are placed, the others none.
fn bounds_of(zone: &Zone, columns: &[usize], places: &[usize]) -> Vec<Option<(Value, Value)>> {
    let mut bounds: Vec<Option<(Value, Value)>> = vec![None; columns.iter().max().map_or(0, |&c| c.saturating_add(1))];
    for (&column, &at) in columns.iter().zip(places) {
        if let (Some(slot), Some(b)) = (bounds.get_mut(column), zone.bounds.get(at)) {
            slot.clone_from(b);
        }
    }
    bounds
}

impl RowTable {
    /// Sums up again some of the zones stale or not known, of the versions
    /// of their rows a reader at or after `horizon` may read, from where
    /// the last left off: whether one is now up to date. One whose rows
    /// have versions a reader may still read besides their newest stays
    /// stale, to be summed up again once none does.
    pub(super) fn sum_zones(&self, horizon: Timestamp) -> Result<bool> {
        let due: Vec<(usize, u64)> = {
            let map = read(&self.zones.map)?;
            let from = self.zones.next_sum.load(AtomicOrdering::Acquire).min(map.zones.len());
            let order = (from..map.zones.len()).chain(0..from);
            let stale = order.filter_map(|at| {
                let zone = lock_builtin(map.zones.get(at)?);
                zone.stale.then_some((at, zone.changes))
            });
            stale.take(MOST_SUMMED).collect()
        };
        let mut done = false;
        for &(at, changes) in &due {
            let summed = self.zone_of(at, horizon)?;
            let map = read(&self.zones.map)?;
            let Some(slot) = map.zones.get(at) else { continue };
            let mut zone = lock_builtin(slot);
            // A write meanwhile: summed again later.
            if zone.changes != changes {
                continue;
            }
            let still = summed.stale;
            *zone = Zone { changes, ..summed };
            if !still {
                self.zones.stale.fetch_sub(1, AtomicOrdering::AcqRel);
                done = true;
            }
        }
        if let Some(&(last, _)) = due.last() {
            self.zones.next_sum.store(last.saturating_add(1), AtomicOrdering::Release);
        }
        Ok(done)
    }

    /// The zone of the rows of zone `at`, summed up from their versions a
    /// reader at or after `horizon` may read: stale still if any is not
    /// its row's newest committed, as a reader of it may end.
    fn zone_of(&self, at: usize, horizon: Timestamp) -> Result<Zone> {
        let first = u64::try_from(at).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
        let (low, high) = (key(first), key(first.saturating_add(ZONE_ROWS)));
        let kept = &self.zones.kept;
        let mut zone = Zone::empty(kept.len(), true);
        zone.stale = false;
        for entry in self.tree.scan(Bound::Included(&low[..]), Bound::Excluded(&high[..]), Direction::Forward) {
            let (k, newest) = entry?;
            self.hold_versions(&mut zone, row_id(&k)?, newest, horizon)?;
        }
        Ok(zone)
    }

    /// Widens `zone` to hold each version of row `id`, whose newest is
    /// `newest`, a reader at or after `horizon` may read: stale if one is
    /// not its newest.
    fn hold_versions(&self, zone: &mut Zone, id: RowId, newest: Vec<u8>, horizon: Timestamp) -> Result<()> {
        let kept = &self.zones.kept;
        let mut bytes = newest;
        loop {
            let older = {
                let version = Version::parse(&bytes)?;
                if !version.deleted {
                    let row = self.decode(id, version.record, Some(kept))?;
                    zone.hold(kept.iter().map(|&c| row.get(c).unwrap_or(&Value::Null)));
                }
                // Versions before one committed at or before the horizon
                // no reader reads.
                match self.maker(&version) {
                    Maker::Committed(stamp) => version.older.filter(|_| stamp > horizon),
                    Maker::Open(_) => version.older,
                }
            };
            zone.stale |= older.is_some();
            let Some(older) = older else { return Ok(()) };
            bytes = self.undo.read(older)?;
        }
    }
}

// Unit tests: zones skip blocks, keep old versions for open readers, and
// are known only once summed up after a table is opened (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
