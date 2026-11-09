//! \[DESIGN\] Of the zones (see the module's documentation), whether every
//! reader sees a zone's rows alone, for index-only scans (RULES.md D34):
//! the flags readers look at, and how a sum up settles them.

use std::sync::atomic::Ordering as AtomicOrdering;

use std::ops::Bound;

use super::super::version::{Maker, Version};
use super::super::{RowTable, key};
use super::{Map, SAMPLED, ZONE_ROWS, Zone, Zones, zone_at};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::RowId;
use crate::sync::{lock_builtin, read};
use crate::txn::Timestamp;

/// What a zone's rows show of whether every reader sees them alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Seen {
    /// Each newest version committed at or before the horizon, none a
    /// deletion.
    #[default]
    Visible,
    /// One not committed, or after the horizon: to be looked at again.
    Recent,
    /// One a deletion: until collection takes it out, or a write.
    Deleted,
}

impl Seen {
    /// What a zone of rows showing `self` and a row showing `other` shows.
    pub(super) fn and(self, other: Seen) -> Seen {
        match (self, other) {
            (Seen::Recent, _) | (_, Seen::Recent) => Seen::Recent,
            (Seen::Deleted, _) | (_, Seen::Deleted) => Seen::Deleted,
            (Seen::Visible, Seen::Visible) => Seen::Visible,
        }
    }
}

impl Zones {
    /// Whether every row of the zone of row `id` is visible to every reader
    /// (see the module's documentation).
    pub(in crate::engine::rows) fn visible(&self, id: RowId) -> bool {
        read(&self.map).is_ok_and(|map| map.visible.get(zone_at(id)).is_some_and(|flag| flag.load(AtomicOrdering::Acquire)))
    }

    /// The share of the zones visible to every reader, of `SAMPLED` at
    /// most, evenly apart; none of no zone.
    pub(in crate::engine::rows) fn visible_share(&self) -> f64 {
        let Ok(map) = read(&self.map) else { return 0.0_f64 };
        let step = map.visible.len().div_ceil(SAMPLED).max(1);
        let (mut all, mut seen) = (0_u32, 0_u32);
        for flag in map.visible.iter().step_by(step) {
            all = all.saturating_add(1);
            seen = seen.saturating_add(u32::from(flag.load(AtomicOrdering::Acquire)));
        }
        if all == 0 { 0.0_f64 } else { f64::from(seen) / f64::from(all) }
    }

    /// Notes a version of row `id` superseded by a commit, to be collected
    /// if `added`, else collected, with its index entries: the zone
    /// unsettled again once none is left.
    pub(in crate::engine::rows) fn superseded(&self, id: RowId, added: bool) {
        let at = zone_at(id);
        let left = read(&self.map).ok().and_then(|map| {
            let count = map.pending.get(at)?;
            Some(if added {
                count.fetch_add(1, AtomicOrdering::AcqRel).saturating_add(1)
            } else {
                count.fetch_sub(1, AtomicOrdering::AcqRel).saturating_sub(1)
            })
        });
        if left == Some(0) {
            self.unsettle(id);
        }
    }

    /// Marks the zone of row `id` unsettled, to be settled again.
    pub(in crate::engine::rows) fn unsettle(&self, id: RowId) {
        let at = zone_at(id);
        let Ok(map) = read(&self.map) else { return };
        let Some(slot) = map.zones.get(at) else { return };
        let mut zone = lock_builtin(slot);
        if !zone.unsettled {
            zone.unsettled = true;
            self.unsettled.fetch_add(1, AtomicOrdering::AcqRel);
        }
    }
}

impl RowTable {
    /// Settles `zone`, zone `at` of `map`, under its lock, its rows showing
    /// `seen`: visible if they are and none of their superseded versions is
    /// left to collect; left unsettled of a commit too recent; else not
    /// visible, until collection or a write unsettles it. Whether settled.
    pub(super) fn settle(&self, (map, at): (&Map, usize), zone: &mut Zone, seen: Seen) -> bool {
        if seen == Seen::Recent {
            return false;
        }
        let collected = map.pending.get(at).is_some_and(|p| p.load(AtomicOrdering::Acquire) == 0);
        if let Some(flag) = map.visible.get(at) {
            flag.store(seen == Seen::Visible && collected, AtomicOrdering::Release);
        }
        zone.unsettled = false;
        self.zones.unsettled.fetch_sub(1, AtomicOrdering::AcqRel);
        true
    }

    /// What the newest versions of the rows of zone `at`, not stale, show
    /// of their visibility to a reader at or after `horizon`, and how many
    /// are no deletion: a zone settled so, its bounds kept, decodes no row
    /// and reads no older version.
    pub(super) fn settled_of(&self, at: usize, horizon: Timestamp) -> Result<(Seen, u32)> {
        let first = u64::try_from(at).unwrap_or(u64::MAX).saturating_mul(ZONE_ROWS);
        let (low, high) = (key(first), key(first.saturating_add(ZONE_ROWS)));
        let (mut seen, mut rows) = (Seen::Visible, 0_u32);
        for entry in self.tree.scan(Bound::Included(&low[..]), Bound::Excluded(&high[..]), Direction::Forward) {
            let (_, newest) = entry?;
            seen = seen.and(self.seen_of(&newest, horizon)?);
            rows = rows.saturating_add(u32::from(!Version::parse(&newest)?.deleted));
        }
        Ok((seen, rows))
    }

    /// What the newest version of a row, `newest`, shows of its
    /// visibility to a reader at or after `horizon` (`Seen`).
    pub(super) fn seen_of(&self, newest: &[u8], horizon: Timestamp) -> Result<Seen> {
        let version = Version::parse(newest)?;
        Ok(match self.maker(&version) {
            Maker::Open(_) => Seen::Recent,
            Maker::Committed(stamp) => {
                if stamp > horizon {
                    Seen::Recent
                } else if version.deleted {
                    Seen::Deleted
                } else {
                    Seen::Visible
                }
            }
        })
    }
}
