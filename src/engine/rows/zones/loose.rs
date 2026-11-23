//! \[DESIGN\] Of the zones (see the module's documentation), which of a
//! zone's columns may be wider than its rows (RULES.md D770): a write
//! loosens only the columns whose bound the value it replaced may have
//! been all that reached, and a sum up decodes only those columns of its
//! rows' newest versions, one value at a time and with no row made,
//! keeping the other columns' bounds. A zone a write stales for one column is summed
//! up for that column alone, so the sum costs what the writes changed.

use std::cmp::Ordering;

use super::{Replaced, Zone, Zones, widened};
use crate::storage::Row;
use crate::value::{Value, compare};

/// \[DESIGN\] The last bit of a zone's loose columns: it stands for the
/// column kept at its place and for every one past it, so a table keeping
/// more columns than a word has bits sums those up together.
const LAST: u32 = u64::BITS.saturating_sub(1);

/// Every column loose: of a zone not known, or a change whose values are
/// not.
pub(super) const ALL: u64 = u64::MAX;

/// The bit of the column kept at `place`.
pub(super) fn bit(place: usize) -> u64 {
    let shift = u32::try_from(place).unwrap_or(LAST).min(LAST);
    1_u64.checked_shl(shift).unwrap_or(0)
}

/// The bit of the column kept at `place` alone, none of one past a word's
/// bits, which `bit` gives the last together.
fn own_bit(place: usize) -> u64 {
    u32::try_from(place).ok().and_then(|shift| 1_u64.checked_shl(shift)).unwrap_or(0)
}

/// \[DESIGN\] Whether each of a zone's `bounds` is still reached once a
/// row's value `was` is replaced by `now`, none of a deletion: a bound
/// `was` lies strictly inside of, or one `now` passes, as the zone is
/// widened to hold it. A value growing past the greatest, as a
/// count or a running total does, then leaves its zone as narrow as its
/// rows, not stale (RULES.md D476).
fn reached((least, greatest): &(Value, Value), was: &Value, now: Option<&Value>) -> bool {
    let moved = now.and_then(|now| compare(now, was));
    let low = compare(least, was) == Some(Ordering::Less) || moved == Some(Ordering::Less);
    let high = compare(was, greatest) == Some(Ordering::Less) || moved == Some(Ordering::Greater);
    low && high
}

impl Zone {
    /// Widens it to hold `values`, those of the `columns` kept, of a row
    /// written over a version that held the same value in each column of
    /// `unchanged`, a bit each by its place. A zone known holds its rows'
    /// newest versions, so the value replaced lies within its bounds, and
    /// a column that keeps it is not compared with them again: an update
    /// costs the comparisons of the columns it changed.
    pub(super) fn hold_changed<'v>(&mut self, columns: usize, values: impl Iterator<Item = &'v Value>, unchanged: u64) {
        let skipped = if self.known { unchanged } else { 0 };
        if self.bounds.len() < columns {
            self.bounds.resize(columns, None);
            self.nulls.resize(columns, false);
        }
        let held = self.bounds.iter_mut().zip(self.nulls.iter_mut()).zip(values).enumerate();
        for (_, ((bounds, null), v)) in held.filter(|&(place, _)| skipped & own_bit(place) == 0) {
            if v.is_null() {
                *null = true;
            } else {
                widened(bounds, v);
            }
        }
    }

    /// Widens it to hold `v`, a value of the column kept at `place`, of
    /// `columns` kept.
    pub(super) fn hold_one(&mut self, columns: usize, place: usize, v: &Value) {
        if self.bounds.len() < columns {
            self.bounds.resize(columns, None);
            self.nulls.resize(columns, false);
        }
        if v.is_null() {
            if let Some(null) = self.nulls.get_mut(place) {
                *null = true;
            }
        } else if let Some(bounds) = self.bounds.get_mut(place) {
            widened(bounds, v);
        }
    }
}

impl Zones {
    /// The columns kept that `loose` names, each with its place.
    pub(super) fn loose_columns(&self, loose: u64) -> Vec<(usize, usize)> {
        self.kept.iter().copied().enumerate().filter(|&(place, _)| loose & bit(place) != 0).collect()
    }

    /// The columns of `zone` that `row`, or a deletion, written over what
    /// was `replaced`, may leave wider than its rows, of those not loose
    /// already: of each column kept, unless the replaced value is the new
    /// one, or each bound is still reached (`reached`), so the one null, if
    /// any, is still there too. A column not loose has each bound reached
    /// by a row's newest version, as `reached` relies on. A zone keeping no
    /// column has no bound to narrow. With them, the columns whose replaced
    /// value is the new one (`Zone::hold_changed`).
    pub(super) fn loosened(&self, zone: &Zone, row: Option<&Row>, replaced: Replaced<'_>) -> (u64, u64) {
        if self.kept.is_empty() {
            return (0, 0);
        }
        let (old, within) = match replaced {
            Replaced::Nothing => return (0, 0),
            Replaced::Unknown => return (ALL, 0),
            Replaced::Row(old) => (old, true),
            Replaced::Outside(old) => (old, false),
        };
        let (mut loose, mut unchanged) = (0_u64, 0_u64);
        for (place, (&c, bounds)) in self.kept.iter().zip(&zone.bounds).enumerate() {
            let was = old.get(c).unwrap_or(&Value::Null);
            let now = row.and_then(|r| r.get(c));
            let same = now.is_some_and(|now| (was.is_null() && now.is_null()) || compare(was, now) == Some(Ordering::Equal));
            if same && within {
                unchanged |= own_bit(place);
            }
            if zone.loose & bit(place) == 0 && !same && !bounds.as_ref().is_some_and(|bounds| reached(bounds, was, now)) {
                loose |= bit(place);
            }
        }
        (loose, unchanged)
    }

    /// Makes `summed`, the sum up of the columns `loose` of `zone`, the
    /// zone's: those columns' bounds and nulls replaced by the sum's, the
    /// others' kept, and its rows and looseness as the sum found them:
    /// none loose, as it read every row's only readable version.
    pub(super) fn apply(&self, zone: &mut Zone, mut summed: Zone, loose: u64) {
        let columns = self.kept.len();
        if (0..columns).all(|place| loose & bit(place) != 0) {
            zone.bounds = summed.bounds;
            zone.nulls = summed.nulls;
        } else if !summed.bounds.is_empty() || !zone.bounds.is_empty() {
            for held in [&mut *zone, &mut summed] {
                held.bounds.resize(columns, None);
                held.nulls.resize(columns, false);
            }
            for (place, _) in self.loose_columns(loose) {
                if let (Some(to), Some(from)) = (zone.bounds.get_mut(place), summed.bounds.get_mut(place)) {
                    *to = from.take();
                }
                if let (Some(to), Some(from)) = (zone.nulls.get_mut(place), summed.nulls.get(place)) {
                    *to = *from;
                }
            }
        }
        zone.rows = summed.rows;
        zone.loose = summed.loose;
        zone.known = true;
        zone.seen = summed.seen;
    }
}
