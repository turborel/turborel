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
/// \[DESIGN\] The most zones the planner looks at for a scan's share.
const SAMPLED: usize = 256;
/// \[DESIGN\] The most zones one call of the maintenance thread sums up.
const MOST_SUMMED: usize = 64;

/// A zone: of each column kept, its least and greatest value not null and
/// whether one is null.
#[derive(Debug, Clone, Default)]
struct Zone {
    bounds: Vec<Option<(Value, Value)>>,
    nulls: Vec<bool>,
    /// Its rows, as inserts count them and a sum up counts the newest: a
    /// deletion counts none until its zone is summed up.
    rows: u32,
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
        Zone { bounds: vec![None; columns], nulls: vec![false; columns], rows: 0, changes: 0, stale: !known, known }
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
        if !replaces {
            zone.rows = zone.rows.saturating_add(1);
        }
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
        let tests = prepared(conditions, &self.kept);
        let mut ranges: Vec<(RowId, Option<RowId>)> = Vec::new();
        for (at, zone) in map.zones.iter().enumerate() {
            if !passes(&lock_builtin(zone), &tests) {
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

    /// The share of the rows a scan told `conditions` reads, by the rows of
    /// the zones it reads, those not known among them, as blocks of row ids
    /// are not as full as one another (`TableStorage::scan_share`). Of
    /// `SAMPLED` zones at most, evenly apart, so planning costs no more as
    /// a table grows; all read if none is known to hold a row.
    pub(super) fn share(&self, conditions: &Conjunction) -> f64 {
        let Ok(map) = read(&self.map) else { return 1.0_f64 };
        if self.kept.is_empty() || map.zones.is_empty() {
            return 1.0_f64;
        }
        let tests = prepared(conditions, &self.kept);
        let step = map.zones.len().div_ceil(SAMPLED).max(1);
        let (mut all, mut read_rows) = (0.0_f64, 0.0_f64);
        for slot in map.zones.iter().step_by(step) {
            let zone = lock_builtin(slot);
            // A zone not known holds as many rows as a block may.
            let rows = if zone.known { f64::from(zone.rows) } else { f64::from(u32::try_from(ZONE_ROWS).unwrap_or(u32::MAX)) };
            all += rows;
            if passes(&zone, &tests) {
                read_rows += rows;
            }
        }
        if all > 0.0_f64 { read_rows / all } else { 1.0_f64 }
    }

    /// Whether a zone is stale or not known.
    pub(super) fn due(&self) -> bool {
        self.stale.load(AtomicOrdering::Acquire) > 0
    }
}

/// How a zone is tested for one conjunct, prepared once for every zone.
enum Test<'c> {
    /// It reads a column not kept: every zone may match.
    Any,
    /// Of one column, kept at this place.
    One(&'c Conjunct, usize),
    /// Of several, each kept: each column, and its place.
    Many(&'c Conjunct, Vec<(usize, usize)>),
}

/// The tests of `conditions`, of the columns `kept`.
fn prepared<'c>(conditions: &'c Conjunction, kept: &[usize]) -> Vec<Test<'c>> {
    let place = |column: &usize| kept.iter().position(|k| k == column);
    conditions
        .conjuncts()
        .iter()
        .map(|c| {
            let columns = c.columns();
            let places: Option<Vec<(usize, usize)>> = columns.iter().map(|col| place(col).map(|at| (*col, at))).collect();
            match places.as_deref() {
                None => Test::Any,
                Some(&[(_, at)]) => Test::One(c, at),
                Some(_) => Test::Many(c, places.unwrap_or_default()),
            }
        })
        .collect()
}

/// Whether a row of `zone` may pass each of `tests`: so if the zone is not
/// known.
fn passes(zone: &Zone, tests: &[Test<'_>]) -> bool {
    if !zone.known {
        return true;
    }
    tests.iter().all(|test| match test {
        Test::Any => true,
        Test::One(Conjunct::Null { not: false, .. }, at) => zone.nulls.get(*at).copied().unwrap_or(true),
        Test::One(c, at) => c.may_match_range(zone.bounds.get(*at).and_then(Option::as_ref)),
        Test::Many(c, places) => {
            let width = places.iter().map(|&(column, _)| column).max().map_or(0, |last| last.saturating_add(1));
            let mut bounds: Vec<Option<(Value, Value)>> = vec![None; width];
            for &(column, at) in places {
                if let (Some(slot), Some(b)) = (bounds.get_mut(column), zone.bounds.get(at)) {
                    slot.clone_from(b);
                }
            }
            c.may_match(&bounds)
        }
    })
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
            if !Version::parse(&newest)?.deleted {
                zone.rows = zone.rows.saturating_add(1);
            }
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
