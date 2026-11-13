//! \[DESIGN\] A row store table's zone maps (RULES.md D31): for each block
//! of `ZONE_ROWS` row ids, of each column kept, the least and greatest
//! value not null its rows hold, and whether one is null; so a scan told
//! conditions (`TableStorage::scan_where`) reads only the blocks whose rows
//! they may match. Row ids never move, so a zone is not touched as leaves
//! split or are taken out.
//!
//! A zone covers every version of its rows a snapshot may read: a write
//! widens it once it is made, before it is committed, and nothing else
//! changes it but the database's maintenance thread
//! (`TableStorage::maintain`). That narrows a zone an update or a deletion
//! left wider than its rows, summing it up
//! again from every version of its rows a reader at or after the horizon
//! may read, an open transaction's among them; a write meanwhile counts a
//! change of the zone, and its sum is then thrown away. A zone is never
//! narrower than its rows, at most wider. A write leaves it stale only if
//! the version it replaced may have been all that reached a bound, or the
//! one null: a value it changed or deleted that lay strictly inside the
//! bounds, or one it kept, leaves every bound reached by a row's newest
//! version still (`Zones::narrows`). The older versions it reads no
//! transaction pins for it: a row one of which is released and truncated
//! as it reads is read again (`RowTable::unpinned`).
//!
//! A change undone narrows nothing: a version put back widens its zone as
//! a write does, and a row an insert added, taken out as the insert is
//! undone, leaves its zone stale, to be narrowed by the maintenance
//! thread as a deletion's is.
//!
//! Each zone also knows whether every row of it is visible to every
//! reader (`Map::visible`), as an index-only scan asks
//! (`TableStorage::visible_to_all`, RULES.md D34): every row's newest
//! version committed at or before the horizon, none a deletion, and no
//! version it superseded left to collect, whose index entries would
//! still be there (`Map::pending`). A write makes it not, before its index
//! entries are written, so a reader that finds a
//! new entry finds its zone not visible; and leaves it unsettled, for the
//! maintenance thread to settle as it sums it up, or, a zone not stale,
//! from its rows' newest versions alone, decoding none: visible, or not,
//! or, of a commit too recent for the horizon, again later. Collection
//! leaving none of a zone's versions to collect, or taking out a
//! deletion, unsettles it again; a version whose index entries it could
//! not take back is left to collect until they are
//! (`TableStorage::taken_back`). Kept in memory alone: a table opened
//! has every zone unsettled, of tables that keep no column too.
//!
//! Once a pass of the maintenance thread has looked at every zone due,
//! the zones rest for `REST`, asking for no maintenance, so writes going
//! on cost a pass over the zones they changed a second, not one a write:
//! a zone narrows, and is settled, that much later (`zones/summed.rs`).
//!
//! Kept in memory, and with the table's pages at each checkpoint
//! (`zones/saved.rs`), so a table opened knows the zones its last
//! checkpoint saved, those it did not save read whole until the
//! maintenance thread has summed each up. Kept are numbers, datetimes,
//! intervals, and character strings of at most `SHORT` characters, so a
//! zone takes a few bytes of each row.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Mutex, RwLock};
use std::time::Instant;

use super::RowTable;
use super::version::Version;
use crate::engine::record::RecordFormat;
use crate::error::Result;
use crate::storage::{Conjunct, Conjunction, Row, RowId};
use crate::sync::{lock_builtin, read, write};
use crate::value::{DataType, Value, compare};

/// \[DESIGN\] The row ids of a zone: a thread's block of them is four
/// (`rows/ids.rs`).
pub(super) const ZONE_ROWS: u64 = 1024;

/// The place of the zone of row `id`.
fn zone_at(id: RowId) -> usize {
    usize::try_from(id.checked_div(ZONE_ROWS).unwrap_or(0)).unwrap_or(usize::MAX)
}
/// \[DESIGN\] The longest character string a zone keeps the bounds of.
const SHORT: u32 = 32;
/// \[DESIGN\] The most zones the planner looks at for a scan's share.
const SAMPLED: usize = 256;
/// \[DESIGN\] The most zones one call of the maintenance thread sums up.
const MOST_SUMMED: usize = 64;

/// What a write replaced of a row, for its zone (`Zones::widen`).
#[derive(Debug, Clone, Copy)]
pub(super) enum Replaced<'r> {
    /// Nothing: the row is new.
    Nothing,
    /// A version whose values are not known, so the zone may be wider
    /// than its rows from now.
    Unknown,
    /// The row's newest version, holding these values of the columns kept.
    Row(&'r Row),
}

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
    /// Whether its rows' visibility is to be settled (`Map::visible`).
    unsettled: bool,
    /// Of a zone summed up, what its rows showed of their visibility.
    seen: Seen,
}

/// The zones of a table, by the first row id of each over `ZONE_ROWS`,
/// each locked alone, so writers of different blocks of row ids, as
/// threads are given (`rows/ids.rs`), do not wait for one another.
#[derive(Default)]
struct Map {
    zones: Vec<Mutex<Zone>>,
    /// Zones made for row ids below this are not known until summed up.
    unknown_below: RowId,
    /// Of each `SEGMENT_ZONES` zones, whether one changed since they were
    /// last saved (`zones/saved.rs`).
    dirty: Vec<AtomicBool>,
    /// Of each zone, whether every row of it is visible to every reader,
    /// read without its lock.
    visible: Vec<AtomicBool>,
    /// Of each zone, the versions its rows' commits superseded not yet
    /// collected, with their index entries.
    pending: Vec<AtomicU64>,
}

impl Map {
    /// Notes zone `at` changed since its segment was saved.
    fn changed(&self, at: usize) {
        if let Some(flag) = self.dirty.get(at.checked_div(SEGMENT_ZONES).unwrap_or(0)) {
            flag.store(true, AtomicOrdering::Release);
        }
    }

    /// Adds the zone `zone`, its segment noted changed: visible if
    /// settled, as one of no row is.
    fn push(&mut self, zone: Zone) {
        self.visible.push(AtomicBool::new(!zone.unsettled));
        self.zones.push(Mutex::new(zone));
        self.pending.push(AtomicU64::new(0));
        while self.dirty.len().saturating_mul(SEGMENT_ZONES) < self.zones.len() {
            self.dirty.push(AtomicBool::new(true));
        }
        self.changed(self.zones.len().saturating_sub(1));
    }
}

/// \[DESIGN\] A table's zone maps.
pub(super) struct Zones {
    /// The columns kept, by their places in a row.
    kept: Vec<usize>,
    /// Locked to be written only to add zones.
    map: RwLock<Map>,
    /// The zones stale or not known.
    stale: AtomicUsize,
    /// The zones unsettled.
    unsettled: AtomicUsize,
    /// The zone a sum up looks from, so zones a reader keeps stale do not
    /// keep the others waiting.
    next_sum: AtomicUsize,
    /// When the zones were made, and the nanoseconds after it until which
    /// they are not due, 0 for none (`REST`).
    born: Instant,
    rest_until: AtomicU64,
    /// The records of a zone's least and greatest values, of the columns
    /// kept, as they are saved.
    format: RecordFormat,
    /// Where they are saved, if they are: not for a table that keeps none,
    /// nor once it is dropped (`zones/saved.rs`).
    place: Mutex<Option<Place>>,
    /// The page listing the pages they are saved in, 0 for none: its meta
    /// page names it.
    directory: u64,
}

/// Whether a table of columns of `types` keeps zones of any, unless it
/// keeps none at all.
pub(super) fn keeps_any(types: &[DataType]) -> bool {
    types.iter().any(kept)
}

/// Whether a column of type `t` is kept.
fn kept(t: &DataType) -> bool {
    match t {
        DataType::Character { length, .. } | DataType::Varying { length, .. } => *length <= SHORT,
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_) => true,
        DataType::Distinct(d) => kept(&d.source),
        DataType::Binary { .. } | DataType::VarBinary { .. } | DataType::Array(_) | DataType::Row(_) | DataType::Extension(_) => false,
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
    /// A zone of no row, known or not, of `columns` kept: settled, of no
    /// row, if known; else to be summed up and settled.
    fn empty(columns: usize, known: bool) -> Zone {
        Zone {
            bounds: vec![None; columns],
            nulls: vec![false; columns],
            rows: 0,
            changes: 0,
            stale: !known,
            known,
            unsettled: !known,
            seen: Seen::Visible,
        }
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
    /// kept unless `keeping`; saved at `place`, if given, its directory
    /// page.
    pub(super) fn new(types: &[DataType], keeping: bool, place: Option<Place>) -> Zones {
        let kept_columns: Vec<usize> =
            if keeping { types.iter().enumerate().filter(|(_, t)| kept(t)).map(|(c, _)| c).collect() } else { Vec::new() };
        let kept_types: Vec<DataType> = kept_columns.iter().filter_map(|&c| types.get(c).cloned()).collect();
        let place = place.filter(|_| !kept_columns.is_empty());
        Zones {
            kept: kept_columns,
            map: RwLock::new(Map::default()),
            stale: AtomicUsize::new(0),
            unsettled: AtomicUsize::new(0),
            next_sum: AtomicUsize::new(0),
            born: Instant::now(),
            rest_until: AtomicU64::new(0),
            format: RecordFormat::new(&kept_types),
            directory: place.as_ref().map_or(0, |p| p.directory.0),
            place: Mutex::new(place),
        }
    }

    /// The page listing the pages they are saved in, 0 for none.
    pub(super) fn directory(&self) -> u64 {
        self.directory
    }

    /// Widens the zone of row `id` to hold `row`, once it is written, the
    /// row's version it `replaced` taken out of it: marked stale if that
    /// may leave it wider than its rows (`Zones::narrows`).
    pub(super) fn widen(&self, id: RowId, row: Option<&Row>, replaced: Replaced<'_>) {
        let at = zone_at(id);
        if let Ok(map) = read(&self.map)
            && let Some(zone) = map.zones.get(at)
        {
            self.change((&map, at), &mut lock_builtin(zone), (row, replaced));
            map.changed(at);
            return;
        }
        let Ok(mut map) = write(&self.map) else { return };
        while map.zones.len() <= at {
            let first = u64::try_from(map.zones.len()).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
            let known = first >= map.unknown_below;
            if !known {
                self.stale.fetch_add(1, AtomicOrdering::AcqRel);
                self.unsettled.fetch_add(1, AtomicOrdering::AcqRel);
            }
            map.push(Zone::empty(self.kept.len(), known));
        }
        if let Some(zone) = map.zones.get(at) {
            self.change((&map, at), &mut lock_builtin(zone), (row, replaced));
        }
        map.changed(at);
    }

    /// Marks the zone of row `id` stale, an insert of it undone: the row
    /// taken out, the zone may be wider than its rows.
    pub(super) fn undone(&self, id: RowId) {
        self.widen(id, None, Replaced::Unknown);
    }

    /// `widen`'s change of `zone`, zone `at` of `map`, under its lock: not
    /// visible from now, before the row's index entries are written, and
    /// unsettled; under the lock a sum up settles it under, so none settles
    /// it visible over the change.
    fn change(&self, (map, at): (&Map, usize), zone: &mut Zone, (row, replaced): (Option<&Row>, Replaced<'_>)) {
        zone.changes = zone.changes.saturating_add(1);
        if let Some(flag) = map.visible.get(at) {
            flag.store(false, AtomicOrdering::Release);
        }
        if !zone.unsettled {
            zone.unsettled = true;
            self.unsettled.fetch_add(1, AtomicOrdering::AcqRel);
        }
        if matches!(replaced, Replaced::Nothing) {
            zone.rows = zone.rows.saturating_add(1);
        }
        let stales = !zone.stale && self.narrows(zone, row, replaced);
        if let Some(row) = row {
            zone.hold(self.kept.iter().map(|&c| row.get(c).unwrap_or(&Value::Null)));
        }
        if stales {
            zone.stale = true;
            self.stale.fetch_add(1, AtomicOrdering::AcqRel);
        }
    }

    /// Whether `zone`, not stale, may be left wider than its rows as `row`,
    /// or a deletion, replaces what was `replaced`: unless, of each column
    /// kept, the replaced value is the new one, or lies strictly inside
    /// the zone's bounds, so some other row still reaches each bound and
    /// the one null, if any, is still there. A zone keeping no column has
    /// no bound to narrow.
    fn narrows(&self, zone: &Zone, row: Option<&Row>, replaced: Replaced<'_>) -> bool {
        let old = match replaced {
            Replaced::Nothing => return false,
            Replaced::Unknown | Replaced::Row(_) if self.kept.is_empty() => return false,
            Replaced::Unknown => return true,
            Replaced::Row(old) => old,
        };
        self.kept.iter().zip(&zone.bounds).any(|(&c, bounds)| {
            let was = old.get(c).unwrap_or(&Value::Null);
            let same = row
                .and_then(|r| r.get(c))
                .is_some_and(|now| (was.is_null() && now.is_null()) || compare(was, now) == Some(Ordering::Equal));
            let inside = bounds.as_ref().is_some_and(|(least, greatest)| {
                compare(least, was) == Some(Ordering::Less) && compare(was, greatest) == Some(Ordering::Less)
            });
            !same && !inside
        })
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

    /// Whether a zone is stale or not known, or unsettled.
    pub(super) fn due(&self) -> bool {
        self.stale.load(AtomicOrdering::Acquire) > 0 || self.unsettled.load(AtomicOrdering::Acquire) > 0
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
    /// Widens the zone of row `id` to hold `row`, or a deletion, written
    /// over the row's newest version `replaced`, if known (`Zones::widen`).
    /// Only the columns kept are decoded of it; one that cannot be is
    /// taken as not known.
    pub(super) fn widen_over(&self, id: RowId, row: Option<&Row>, replaced: Option<&[u8]>) {
        let kept = &self.zones.kept;
        let old = replaced.filter(|_| !kept.is_empty()).and_then(|bytes| {
            let version = Version::parse(bytes).ok().filter(|v| !v.deleted)?;
            self.decode(id, version.record, Some(kept)).ok()
        });
        self.zones.widen(id, row, old.as_ref().map_or(Replaced::Unknown, Replaced::Row));
    }

    /// Widens the zone of row `id` to hold `version`, a version of it put
    /// back in its tree, as a write widens it once written: a zone summed
    /// up while the version was not the row's newest holds it again.
    pub(super) fn widen_to(&self, id: RowId, version: &[u8]) -> Result<()> {
        let version = Version::parse(version)?;
        let row = if version.deleted { None } else { Some(self.decode(id, version.record, Some(&self.zones.kept))?) };
        self.zones.widen(id, row.as_ref(), Replaced::Unknown);
        Ok(())
    }
}

mod saved;
mod summed;
mod visible;

pub(super) use saved::Place;
use saved::SEGMENT_ZONES;
use visible::Seen;

// Unit tests: zones skip blocks, keep old versions for open readers, are
// known only once summed up after a table is opened or as a checkpoint
// saved them, hold every row written while they are summed up, hold a
// version put back, are narrowed after an insert is undone, and are
// summed up as the undo store is truncated (RULES.md R-TEST-1, R-TEST-7).
#[cfg(test)]
mod tests;
