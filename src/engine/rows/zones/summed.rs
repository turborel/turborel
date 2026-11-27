//! \[DESIGN\] Of the zones (see the module's documentation), how the
//! maintenance thread sums them up and settles them, and how they rest
//! after a pass over them: a zone stale summed up from its rows' newest
//! versions once no reader may read an older one, one unsettled alone
//! settled from its rows' newest versions, and, once every zone due was looked at, no zone due
//! for `REST`, so writes going on cost a pass a second. A zone due again
//! soon after it was looked at is hot, and waits longer before it is
//! looked at again, up to `MOST_WAIT` (RULES.md D771).

use std::ops::Bound;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::Duration;

use super::super::version::Version;
use super::super::{RowTable, key, row_id};
use super::visible::Seen;
use super::{MOST_SUMMED, ZONE_ROWS, Zone, Zones};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::sync::lock_builtin;
use crate::txn::Timestamp;

/// \[DESIGN\] How long a table's zones, each summed up or settled, are
/// not due again, so writes going on cost the maintenance thread a pass
/// over the zones they changed a second, not one a write.
pub(super) const REST: Duration = Duration::from_secs(1);

/// \[DESIGN\] The longest a hot zone waits after it was looked at, so a
/// zone written without pause is summed up once in as long, not once a
/// `REST`, its narrowing and settling as much later.
pub(super) const MOST_WAIT: Duration = Duration::from_secs(32);

/// \[DESIGN\] A zone looked at again within this many of its waits since
/// it was last, `REST` at least, is hot: due again about as soon as its
/// wait let it be.
pub(super) const HOT: u64 = 4;

/// The nanoseconds of `d`.
pub(super) fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

impl Zone {
    /// Whether its wait since it was last looked at is over, `elapsed`
    /// nanoseconds after the zones were made.
    pub(super) fn ready(&self, elapsed: u64) -> bool {
        self.looked.is_none_or(|looked| elapsed >= looked.saturating_add(self.wait))
    }

    /// Notes it looked at, `elapsed` nanoseconds after the zones were
    /// made: its wait doubled, from `REST` to `MOST_WAIT`, if it is hot;
    /// else none, so a zone written now and then is looked at a `REST`
    /// after.
    pub(super) fn looked_at(&mut self, elapsed: u64) {
        let window = self.wait.max(nanos(REST)).saturating_mul(HOT);
        let hot = self.looked.is_some_and(|looked| elapsed.saturating_sub(looked) < window);
        self.wait = if hot { self.wait.saturating_mul(2).clamp(nanos(REST), nanos(MOST_WAIT)) } else { 0 };
        self.looked = Some(elapsed);
    }
}

impl Zones {
    /// Whether a zone is due, one was asked for since the table was made
    /// or opened (RULES.md D1212), and the zones are not resting after a
    /// pass over them (`REST`), so the maintenance thread is to be asked
    /// for.
    pub(in crate::engine::rows) fn wanted(&self) -> bool {
        self.wanted_at(self.elapsed())
    }

    /// `wanted` at `clock` nanoseconds after the zones were made.
    pub(in crate::engine::rows) fn wanted_at(&self, clock: u64) -> bool {
        self.asked() && self.due() && clock >= self.rest_until.load(AtomicOrdering::Acquire)
    }

    /// The nanoseconds since the zones were made.
    fn elapsed(&self) -> u64 {
        nanos(self.born.elapsed())
    }

    /// Notes `looked` more zones looked at, of `zones`: whether every zone
    /// has been since the zones last rested, as a pass over them is then
    /// done, though as many as `MOST_SUMMED` were due again in it. Zones
    /// writes keep stale, their rows' older versions readable as each is
    /// summed up, are then summed up once a `REST`, not over and over.
    fn swept(&self, looked: usize, zones: usize) -> bool {
        let swept = self.swept.fetch_add(looked, AtomicOrdering::AcqRel).saturating_add(looked);
        swept >= zones
    }

    /// The next zones due, `MOST_SUMMED` at most, from where the last sum
    /// up left off, past those still waiting at `clock` nanoseconds after
    /// the zones were made, if given: each with its changes, and its loose
    /// columns. The zones rest if those are all, or once every zone has
    /// been looked at since they last rested (`swept`).
    fn due_zones(&self, clock: Option<u64>) -> Vec<(usize, u64, u64)> {
        let zones = self.slots.len();
        let from = self.next_sum.load(AtomicOrdering::Acquire).min(zones);
        let mut looked = 0_usize;
        let order = (from..zones).chain(0..from).inspect(|_| looked = looked.saturating_add(1));
        let stale = order.filter_map(|at| {
            let zone = lock_builtin(&self.slots.get(at)?.zone);
            let ready = clock.is_none_or(|clock| zone.ready(clock));
            ((zone.stale() || zone.unsettled) && ready).then(|| (at, zone.changes, zone.loose))
        });
        let due: Vec<(usize, u64, u64)> = stale.take(MOST_SUMMED).collect();
        if self.swept(looked, zones) || due.len() < MOST_SUMMED {
            self.rest(clock.unwrap_or_else(|| self.elapsed()));
        }
        due
    }

    /// Rests the zones for `REST` from `clock` nanoseconds after they were
    /// made, a pass over every zone due done.
    fn rest(&self, clock: u64) {
        self.swept.store(0, AtomicOrdering::Release);
        self.rest_until.store(clock.saturating_add(nanos(REST)), AtomicOrdering::Release);
    }
}

impl RowTable {
    /// Sums up again some of the zones stale or not known, or unsettled,
    /// of the versions of their rows a reader at or after `horizon` may
    /// read, from where the last left off: whether one is now up to date,
    /// or settled. One whose rows have versions a reader may still read
    /// besides their newest is left as it is, stale, to be summed up once
    /// none does; one of a commit too recent stays unsettled. One unsettled
    /// alone is settled from its rows' newest versions, its bounds kept
    /// (`RowTable::settled_of`); one stale summed up of its loose columns
    /// alone (`zones/loose.rs`). Once every zone due was looked at, or every
    /// zone since they last rested, the zones rest (`REST`). A zone still
    /// waiting after it was last looked at is passed over, unless `now`.
    pub(in crate::engine::rows) fn sum_zones(&self, horizon: Timestamp, now: bool) -> Result<bool> {
        self.sum_zones_at(horizon, (!now).then(|| self.zones.elapsed()))
    }

    /// `sum_zones` at `clock` nanoseconds after the zones were made, the
    /// zones still waiting then passed over; every zone due, if not given.
    pub(in crate::engine::rows) fn sum_zones_at(&self, horizon: Timestamp, clock: Option<u64>) -> Result<bool> {
        let due = self.zones.due_zones(clock);
        let looked = clock.unwrap_or_else(|| self.zones.elapsed());
        let mut done = false;
        for &(at, changes, loose) in &due {
            let summed = if loose == 0 { None } else { Some(self.zone_of(at, horizon, loose)?) };
            let (seen, rows) = match &summed {
                Some(Some(summed)) => (summed.seen, summed.rows),
                Some(None) => (Seen::Recent, 0),
                None => self.settled_of(at, horizon)?,
            };
            let Some(slot) = self.zones.slots.get(at) else { continue };
            let mut zone = lock_builtin(&slot.zone);
            zone.looked_at(looked);
            // A write meanwhile: summed again later. None, a zone not stale
            // is not stale still.
            if zone.changes != changes {
                continue;
            }
            let (was_stale, unsettled) = (zone.stale(), zone.unsettled);
            match summed {
                Some(Some(summed)) => self.zones.apply(&mut zone, summed, loose),
                Some(None) => {}
                None => zone.rows = rows,
            }
            self.zones.changed(at);
            if was_stale && !zone.stale() {
                self.zones.stale.fetch_sub(1, AtomicOrdering::AcqRel);
                done = true;
            }
            if unsettled && self.settle(slot, &mut zone, seen) {
                done = true;
            }
        }
        if let Some(&(last, ..)) = due.last() {
            self.zones.next_sum.store(last.saturating_add(1), AtomicOrdering::Release);
        }
        Ok(done)
    }

    /// The zone of the rows of zone `at`, of its columns `loose`, summed up
    /// from their newest versions, each column decoded by itself, if every
    /// one is committed at or before `horizon`, or the table's latest
    /// collection if later, so no reader reads an older
    /// version; none if one is not, the zone left as wide as the writes
    /// widened it, to be narrowed once the horizon has passed them. No
    /// older version is read back (RULES.md D770). What the newest versions
    /// show of their visibility, too.
    fn zone_of(&self, at: usize, horizon: Timestamp, loose: u64) -> Result<Option<Zone>> {
        let first = u64::try_from(at).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
        let (low, high) = (key(first), key(first.saturating_add(ZONE_ROWS)));
        let (columns, kept) = (self.zones.loose_columns(loose), self.zones.kept.len());
        let (mut zone, horizon) = (Zone::empty(true), self.collected_horizon(horizon));
        for entry in self.tree.scan(Bound::Included(&low[..]), Bound::Excluded(&high[..]), Direction::Forward) {
            let (k, newest) = entry?;
            let seen = self.seen_of(&newest, horizon)?;
            if seen == Seen::Recent {
                return Ok(None);
            }
            zone.seen = zone.seen.and(seen);
            let version = Version::parse(&newest)?;
            if !version.deleted {
                zone.rows = zone.rows.saturating_add(1);
                self.each_column((row_id(&k)?, version.record), &columns, &mut |place, v| zone.hold_one(kept, place, v))?;
            }
        }
        Ok(Some(zone))
    }
}
