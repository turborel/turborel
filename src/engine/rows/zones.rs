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
//! left wider than its rows, summing it up again from its rows' newest
//! versions once each is committed at or before the horizon, so no reader
//! reads an older one: until then the zone stays as wide as the writes
//! widened it, and no older version is read back. A write meanwhile counts
//! a change of the zone, and its sum is then thrown away. A zone is never
//! narrower than its rows, at most wider. A write leaves it stale only if
//! the version it replaced may have been all that reached a bound, or the
//! one null: a value it changed or deleted that lay strictly inside the
//! bounds, one it kept, or one on a bound changed to a value past it, as
//! a running total grows past the greatest, leaves every bound reached by
//! a row's newest version still (`Zones::loosened`, RULES.md D476). It is
//! stale only in the columns it may be wider in, and a sum up reads those
//! columns alone, keeping the others' bounds (`zones/loose.rs`, RULES.md
//! D770).
//!
//! A change undone narrows nothing: a version put back widens its zone as
//! a write does, and a row an insert added, taken out as the insert is
//! undone, leaves its zone stale, to be narrowed by the maintenance
//! thread as a deletion's is.
//!
//! Each zone also knows whether every row of it is visible to every
//! reader (`Slot::visible`), as an index-only scan asks
//! (`TableStorage::visible_to_all`, RULES.md D34): every row's newest
//! version committed at or before the horizon, none a deletion, and no
//! version it superseded left to collect, whose index entries would
//! still be there (`Slot::pending`). A write makes it not, before its index
//! entries are written, so a reader that finds a
//! new entry finds its zone not visible; and leaves it unsettled, for the
//! maintenance thread to settle as it sums it up, or, a zone not stale,
//! from its rows' newest versions alone, decoding none: visible, or not,
//! or, of a commit too recent for the horizon, again later. Collection
//! leaving none of a zone's versions to collect, or taking out a
//! deletion, unsettles it again; a version whose index entries it could
//! not take back is left to collect until they are
//! (`TableStorage::taken_back`). Kept in memory, and with the zone
//! where its segment is saved: a table opened has a zone saved settled
//! visible so, every other unsettled, of tables that keep no column too
//! (`zones/saved.rs`, RULES.md D1163).
//!
//! Once a pass of the maintenance thread has looked at every zone due,
//! the zones rest for `REST`, asking for no maintenance, so writes going
//! on cost a pass over the zones they changed a second, not one a write:
//! a zone narrows, and is settled, that much later (`zones/summed.rs`).
//! A zone due again about as soon as it was looked at waits twice as long
//! each time, to a limit, so a zone written without pause is summed up
//! once in that long (RULES.md D771).
//!
//! Kept in memory, and with the table's pages at each checkpoint
//! (`zones/saved.rs`), so a table opened knows the zones its last
//! checkpoint saved, those it did not save read whole until the
//! maintenance thread has summed each up. That it does only once a zone
//! of the table is asked for after it opens, so a start costs no pass
//! over a table nothing reads (`zones/cold.rs`, RULES.md D1212). Kept are numbers, datetimes,
//! intervals, and character strings of at most `SHORT` characters, so a
//! zone takes a few bytes of each row.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::counted::Live;
use crate::engine::record::RecordFormat;
use crate::storage::{Conjunct, Conjunction, Row, RowId};
use crate::sync::lock_builtin;
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
    /// As `Row`, of a table that has frozen rows or held a group: the
    /// version may have been read from a group, and the zones of such a
    /// table are of its tree's rows alone, so its values are not taken to
    /// lie within the zone's bounds (`Zone::hold_changed`).
    Outside(&'r Row),
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
    /// Of each column kept, by its place, whether its bounds or null may
    /// be wider than its rows, a bit each (`zones/loose.rs`): the zone is
    /// stale if any is, every one if it is not known at all.
    loose: u64,
    known: bool,
    /// Whether its rows' visibility is to be settled (`Slot::visible`).
    unsettled: bool,
    /// Of a zone summed up, what its rows showed of their visibility.
    seen: Seen,
    /// When the maintenance thread last looked at it, in nanoseconds
    /// after the zones were made, and how long after that it is not
    /// looked at again (`zones/summed.rs`).
    looked: Option<u64>,
    wait: u64,
}

/// A zone as its table holds it, locked alone, so writers of different
/// blocks of row ids, as threads are given (`rows/ids.rs`), do not wait for
/// one another; with what is read of it without its lock.
#[derive(Default)]
pub(super) struct Slot {
    zone: Mutex<Zone>,
    /// Whether every row of it is visible to every reader.
    visible: AtomicBool,
    /// The versions its rows' commits superseded not yet collected, with
    /// their index entries.
    pending: AtomicU64,
}

/// \[DESIGN\] A table's zone maps.
pub(super) struct Zones {
    /// The columns kept, by their places in a row.
    kept: Vec<usize>,
    /// The zones, by the first row id of each over `ZONE_ROWS`, found
    /// with no lock (`zones/grown.rs`, RULES.md D860).
    slots: Grown<Slot>,
    /// Of each `SEGMENT_ZONES` zones, whether one changed since they were
    /// last saved (`zones/saved.rs`).
    dirty: Grown<AtomicBool>,
    /// Held while zones are added.
    growing: Mutex<()>,
    /// Zones made for row ids below this are not known until summed up.
    unknown_below: AtomicU64,
    /// Whether zones are kept: not until one is asked for
    /// (`zones/cold.rs`, RULES.md D868).
    warm: AtomicBool,
    /// Whether a zone was asked for since the table was made or opened:
    /// until one is, a table opened has none summed up (RULES.md D1212).
    asked: AtomicBool,
    /// Whether the end of a transaction was told of the ask (`take_ask`).
    told: AtomicBool,
    /// The table's row ids given so far: one past them.
    given: Arc<AtomicU64>,
    /// The versions superseded while the table was cold, not yet collected.
    owed: Live,
    /// The zones stale or not known.
    stale: AtomicUsize,
    /// The zones unsettled.
    unsettled: AtomicUsize,
    /// The zone a sum up looks from, so zones a reader keeps stale do not
    /// keep the others waiting.
    next_sum: AtomicUsize,
    /// The zones looked at since the zones last rested (`REST`).
    swept: AtomicUsize,
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
        DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => false,
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
    /// A zone of no row, known or not: settled, of no row, if known; else
    /// to be summed up and settled. Its bounds and nulls are made as it
    /// first holds a row, so a zone of none, as those of a column store
    /// table's frozen rows are once summed up, takes no memory for them.
    fn empty(known: bool) -> Zone {
        let loose = if known { 0 } else { ALL };
        Zone {
            bounds: Vec::new(),
            nulls: Vec::new(),
            rows: 0,
            changes: 0,
            loose,
            known,
            unsettled: !known,
            seen: Seen::Visible,
            looked: None,
            wait: 0,
        }
    }

    /// Whether it may be wider than its rows, or is not known at all.
    fn stale(&self) -> bool {
        self.loose != 0
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
            slots: Grown::default(),
            dirty: Grown::default(),
            growing: Mutex::new(()),
            unknown_below: AtomicU64::new(0),
            warm: AtomicBool::new(false),
            asked: AtomicBool::new(false),
            told: AtomicBool::new(false),
            given: Arc::default(),
            owed: Live::new(),
            stale: AtomicUsize::new(0),
            unsettled: AtomicUsize::new(0),
            next_sum: AtomicUsize::new(0),
            swept: AtomicUsize::new(0),
            born: Instant::now(),
            rest_until: AtomicU64::new(0),
            format: RecordFormat::new(&kept_types),
            directory: place.as_ref().map_or(0, |p| p.directory.0),
            place: Mutex::new(place),
        }
    }

    /// The zones, of a table whose row ids given so far are below `given`.
    pub(super) fn over(mut self, given: Arc<AtomicU64>) -> Zones {
        self.given = given;
        self
    }

    /// The page listing the pages they are saved in, 0 for none.
    pub(super) fn directory(&self) -> u64 {
        self.directory
    }

    /// Notes zone `at` changed since its segment was saved. The flag is
    /// written only if clear: every write to the table notes a change, and
    /// one flag stands for many zones, so a flag set already is read alone
    /// and its cache line stays shared between the writers. A save clears
    /// it before it reads the zones (`zones/saved.rs`), so a change it did
    /// not read finds it clear and sets it.
    fn changed(&self, at: usize) {
        let flag = self.dirty.get(at.checked_div(SEGMENT_ZONES).unwrap_or(0));
        if let Some(flag) = flag.filter(|flag| !flag.load(AtomicOrdering::Acquire)) {
            flag.store(true, AtomicOrdering::Release);
        }
    }

    /// Adds the zone `zone`, under `growing`, its segment noted changed:
    /// visible if settled, as one of no row is. Whether it was added.
    fn push(&self, zone: Zone) -> bool {
        let pushed = self.slots.push(|slot| {
            slot.visible.store(!zone.unsettled, AtomicOrdering::Release);
            *lock_builtin(&slot.zone) = zone;
        });
        let mut flagged = pushed;
        while flagged && self.dirty.len().saturating_mul(SEGMENT_ZONES) < self.slots.len() {
            flagged = self.dirty.push(|flag| flag.store(true, AtomicOrdering::Release));
        }
        self.changed(self.slots.len().saturating_sub(1));
        pushed
    }

    /// Adds zones until there is a zone `at`, each of no row yet.
    fn grow_to(&self, at: usize) {
        let _growing = lock_builtin(&self.growing);
        let mut added = true;
        while added && self.slots.len() <= at {
            let first = u64::try_from(self.slots.len()).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
            let known = first >= self.unknown_below.load(AtomicOrdering::Acquire);
            added = self.push(Zone::empty(known));
            if added && !known {
                self.stale.fetch_add(1, AtomicOrdering::AcqRel);
                self.unsettled.fetch_add(1, AtomicOrdering::AcqRel);
            }
        }
    }

    /// Widens the zone of row `id` to hold `row`, once it is written, the
    /// row's version it `replaced` taken out of it: its columns marked
    /// loose that this may leave wider than its rows (`Zones::loosened`).
    /// The zone is found with no lock, and locked alone (RULES.md D860).
    /// Of a table cold, nothing is done (`zones/cold.rs`).
    pub(super) fn widen(&self, id: RowId, row: Option<&Row>, replaced: Replaced<'_>) {
        if self.cold() {
            return;
        }
        let at = zone_at(id);
        if self.slots.get(at).is_none() {
            self.grow_to(at);
        }
        let Some(slot) = self.slots.get(at) else { return };
        self.change(slot, &mut lock_builtin(&slot.zone), (row, replaced));
        self.changed(at);
    }

    /// Marks the zone of row `id` stale, an insert of it undone: the row
    /// taken out, the zone may be wider than its rows.
    pub(super) fn undone(&self, id: RowId) {
        self.widen(id, None, Replaced::Unknown);
    }

    /// `widen`'s change of `zone`, the zone of `slot`, under its lock: not
    /// visible from now, before the row's index entries are written, and
    /// unsettled; under the lock a sum up settles it under, so none settles
    /// it visible over the change.
    fn change(&self, slot: &Slot, zone: &mut Zone, (row, replaced): (Option<&Row>, Replaced<'_>)) {
        zone.changes = zone.changes.saturating_add(1);
        // Written only if set, under the zone's lock, as a sum up sets it:
        // a flag clear already is read alone, so writers of zones side by
        // side do not pass its cache line from one to the next.
        let flag = &slot.visible;
        if flag.load(AtomicOrdering::Acquire) {
            flag.store(false, AtomicOrdering::Release);
        }
        if !zone.unsettled {
            zone.unsettled = true;
            self.unsettled.fetch_add(1, AtomicOrdering::AcqRel);
        }
        if matches!(replaced, Replaced::Nothing) {
            zone.rows = zone.rows.saturating_add(1);
        }
        let was_stale = zone.stale();
        let (loose, unchanged) = self.loosened(zone, row, replaced);
        zone.loose |= loose;
        if let Some(row) = row {
            zone.hold_changed(self.kept.len(), self.kept.iter().map(|&c| row.get(c).unwrap_or(&Value::Null)), unchanged);
        }
        if !was_stale && zone.stale() {
            self.stale.fetch_add(1, AtomicOrdering::AcqRel);
        }
    }

    /// The ranges of row ids a scan told `conditions` reads, in order, each
    /// from its first id up to, not including, its end, if it has one.
    pub(super) fn ranges(&self, conditions: Option<&Conjunction>) -> Vec<(RowId, Option<RowId>)> {
        let all = vec![(0, None)];
        let (Some(conditions), false) = (conditions, self.kept.is_empty()) else { return all };
        let tests = prepared(conditions, &self.kept);
        if tests.iter().all(|test| matches!(test, Test::Any)) {
            return all;
        }
        // Asked by a scan they may prune for: kept from now on.
        self.warm();
        let mut ranges: Vec<(RowId, Option<RowId>)> = Vec::new();
        // Of the zones there are now: a row of one added meanwhile is past them.
        let zones = self.slots.len();
        for (at, slot) in self.slots.first(zones).enumerate() {
            if passes(&lock_builtin(&slot.zone), &tests) {
                joined(&mut ranges, at);
            }
        }
        past(&mut ranges, zones);
        ranges
    }

    /// The share of the rows a scan told `conditions` reads, by the rows of
    /// the zones it reads, those not known among them, as blocks of row ids
    /// are not as full as one another (`TableStorage::scan_share`). Of
    /// `SAMPLED` zones at most, evenly apart, so planning costs no more as
    /// a table grows; all read if none is known to hold a row. A share is
    /// never less than one zone of those looked at. The zones a sample
    /// passed over may hold the rows asked for, as the rows of the order
    /// just written lie in one zone (RULES.md D838), and a zone none of
    /// whose rows may match now may hold rows written later, which a plan
    /// kept for later executions reads (D839).
    pub(super) fn share(&self, conditions: &Conjunction) -> f64 {
        let zones = self.slots.len();
        if self.kept.is_empty() || zones == 0 {
            return 1.0_f64;
        }
        let tests = prepared(conditions, &self.kept);
        let step = zones.div_ceil(SAMPLED).max(1);
        let (mut all, mut read_rows, mut sampled) = (0.0_f64, 0.0_f64, 0.0_f64);
        for slot in self.slots.first(zones).step_by(step) {
            sampled += 1.0_f64;
            let zone = lock_builtin(&slot.zone);
            // A zone not known holds as many rows as a block may.
            let rows = if zone.known { f64::from(zone.rows) } else { f64::from(u32::try_from(ZONE_ROWS).unwrap_or(u32::MAX)) };
            all += rows;
            if passes(&zone, &tests) {
                read_rows += rows;
            }
        }
        if all <= 0.0_f64 {
            return 1.0_f64;
        }
        (read_rows / all).max(1.0_f64 / sampled)
    }

    /// Whether a zone is stale or not known, or unsettled.
    pub(super) fn due(&self) -> bool {
        self.stale.load(AtomicOrdering::Acquire) > 0 || self.unsettled.load(AtomicOrdering::Acquire) > 0
    }
}

/// Adds the row ids of zone `at` to `ranges`, joined to the last if it goes
/// on from it.
fn joined(ranges: &mut Vec<(RowId, Option<RowId>)>, at: usize) {
    let first = u64::try_from(at).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
    let end = first.saturating_add(ZONE_ROWS);
    if let Some((_, Some(last))) = ranges.last_mut().filter(|r| r.1 == Some(first)) {
        *last = end;
    } else {
        ranges.push((first, Some(end)));
    }
}

/// Adds to `ranges` the row ids past the last of `zones` zones, of rows
/// written as a scan begins.
fn past(ranges: &mut Vec<(RowId, Option<RowId>)>, zones: usize) {
    let first = u64::try_from(zones).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
    ranges.push((first, None));
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

mod cold;
mod grown;
mod loose;
mod saved;
mod summed;
mod visible;
mod written;

use grown::Grown;
use loose::ALL;
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
// Unit tests: the zones rest after a pass (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod rested;
// Unit tests: a zone is summed up in its loose columns alone, a hot zone
// waits longer, resting zones are left alone unless asked now, and bounds
// hold every row under writes summed up meanwhile (RULES.md R-TEST-1,
// R-TEST-7, R-TEST-8).
#[cfg(test)]
mod costs;
// Unit tests: a table opened has no zone summed up until one is asked
// for (RULES.md R-TEST-1, R-TEST-7, R-TEST-8).
#[cfg(test)]
mod asked;
