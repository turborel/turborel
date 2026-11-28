//! \[DESIGN\] (RULES.md D1120) The states of a set function of few groups
//! fed by lanes. A grouping of few groups, four say, gives one
//! group to many rows in turn, and each row's add to its group's total
//! waits on the store of the row before it. Rows are fed in turn to
//! `LANES` partial states of each group, so the adds of a lane's rows
//! do not wait on those of the others, and the partials are added to the
//! groups' states once the batch is fed. A sum of at most 2^63 numbers
//! below 2^63 each stays below 2^126, so no partial of a batch reaches
//! the 128 bits the states sum in, and the totals are those the rows fed
//! one at a time give.
//!
//! \[DESIGN\] (RULES.md D1308) The partials are as wide as the totals.
//! Narrow totals are fed by lanes of 64 bits. A batch is added to the
//! totals only once every add of it has stayed within their width, so one
//! that leaves 64 bits has added nothing, and is fed again by lanes of
//! 128 bits to the column widened.

use super::Totals;
use crate::storage::mask::index;

/// The most groups fed by lanes: their partials, of 16 bytes a sum at the
/// most and 8 a count, 6 KiB of four lanes, stay beside the batch's
/// vectors.
pub(super) const FEW: usize = 64;

/// The partial states a group is fed by.
const LANES: usize = 4;

/// A batch's rows as lanes are fed them: the integers, and each row's
/// place among them and its group.
type Rows<'a> = (&'a [i64], &'a [u32], &'a [u32]);

/// A total of either width, as lanes add it.
trait Summing: Copy + Default {
    /// A row's integer at this width.
    fn of(unit: i64) -> Self;

    /// The sum with `other`, wrapped, and whether it left the width.
    fn plus(self, other: Self) -> (Self, bool);
}

impl Summing for i64 {
    fn of(unit: i64) -> i64 {
        unit
    }

    fn plus(self, other: i64) -> (i64, bool) {
        self.overflowing_add(other)
    }
}

impl Summing for i128 {
    fn of(unit: i64) -> i128 {
        i128::from(unit)
    }

    fn plus(self, other: i128) -> (i128, bool) {
        self.overflowing_add(other)
    }
}

/// Adds to `totals` and `counts`, a group each, the integers of `rows`,
/// by lanes: whether a total overflowed 128 bits; none, nothing added, of
/// more than `FEW` groups. Narrow totals whose batch leaves 64 bits are
/// widened and fed it again (RULES.md D1308).
pub(super) fn summed(totals: &mut Totals, counts: &mut [i64], rows: Rows<'_>) -> Option<bool> {
    if totals.len() > FEW || counts.len() != totals.len() {
        return None;
    }
    // Whether the batch left the 64 bits of narrow totals, and is yet to
    // be added, as is every batch of wide ones.
    let left = if let Totals::Narrow(narrow) = totals { by_lanes(narrow, rows) } else { true };
    let mut over = false;
    if left {
        totals.widen();
        let Totals::Wide(wide) = totals else { return None };
        over = by_lanes(wide, rows);
    }
    counted(counts, rows.2);
    Some(over)
}

/// Adds to `totals`, a group each, the integers of `rows`, by lanes of
/// the totals' width: whether an add left that width, and then nothing is
/// added.
fn by_lanes<T: Summing>(totals: &mut [T], (units, places, groups): Rows<'_>) -> bool {
    let (mut sums, mut over) = ([[T::default(); FEW]; LANES], false);
    let mut fed = |sum: &mut [T; FEW], (&at, &g): (&u32, &u32)| {
        if let Some(total) = sum.get_mut(index(g)) {
            let unit = units.get(index(at)).copied().unwrap_or(0);
            let (made, overflowed) = total.plus(T::of(unit));
            (*total, over) = (made, over | overflowed);
        }
    };
    let (turns, last) = places.as_chunks::<LANES>();
    let (of_turns, of_last) = groups.as_chunks::<LANES>();
    for (at, of) in turns.iter().zip(of_turns) {
        for (sum, row) in sums.iter_mut().zip(at.iter().zip(of)) {
            fed(sum, row);
        }
    }
    for (sum, row) in sums.iter_mut().zip(last.iter().zip(of_last)) {
        fed(sum, row);
    }
    // The totals as the batch leaves them, kept only if no add left
    // their width.
    let mut made = [T::default(); FEW];
    made.iter_mut().zip(totals.iter()).for_each(|(to, &total)| *to = total);
    for sum in &sums {
        for (total, &part) in made.iter_mut().zip(sum) {
            let (added, overflowed) = total.plus(part);
            (*total, over) = (added, over | overflowed);
        }
    }
    if !over {
        totals.iter_mut().zip(&made).for_each(|(total, &kept)| *total = kept);
    }
    over
}

/// Adds to `counts`, a group each, the rows of groups `groups`, by lanes:
/// whether it did, not of more than `FEW` groups.
pub(super) fn counted(counts: &mut [i64], groups: &[u32]) -> bool {
    if counts.len() > FEW {
        return false;
    }
    let mut partials = [[0_i64; FEW]; LANES];
    let fed = |lane: &mut [i64; FEW], g: u32| {
        if let Some(count) = lane.get_mut(index(g)) {
            *count = count.saturating_add(1);
        }
    };
    let (turns, last) = groups.as_chunks::<LANES>();
    for of in turns.iter().map(<[u32; LANES]>::as_slice).chain([last]) {
        for (lane, &g) in partials.iter_mut().zip(of) {
            fed(lane, g);
        }
    }
    for lane in &partials {
        for (total, &n) in counts.iter_mut().zip(lane) {
            *total = total.saturating_add(n);
        }
    }
    true
}

// Unit tests: totals and counts fed by lanes are those of the rows fed
// one at a time, of a batch whose rows fill no last turn of the lanes
// too (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
