//! \[DESIGN\] (RULES.md D1120) The states of a set function of few groups
//! fed by lanes. A grouping of few groups, four say, gives one
//! group to many rows in turn, and each row's add to its group's total
//! waits on the store of the row before it. Rows are fed in turn to
//! `LANES` partial states of each group, so the adds of a lane's rows
//! do not wait on those of the others, and the partials are added to the
//! groups' states once the batch is fed. A sum of at most 2^63 numbers
//! below 2^63 each stays below 2^126, so no partial nor total reaches
//! the 128 bits the states sum in: the overflow kept is never met, and
//! the totals are those the rows fed one at a time give.

use crate::storage::mask::index;

/// The most groups fed by lanes: their partials, of 16 bytes a sum and
/// 8 a count, 6 KiB of four lanes, stay beside the batch's vectors.
pub(super) const FEW: usize = 64;

/// The partial states a group is fed by.
const LANES: usize = 4;

/// Adds to `totals` and `counts`, a group each, the integers `units` at
/// `places` of the rows of groups `groups`, by lanes: whether a total
/// overflowed; none, nothing added, of more than `FEW` groups.
pub(super) fn summed(totals: &mut [i128], counts: &mut [i64], (units, places, groups): (&[i64], &[u32], &[u32])) -> Option<bool> {
    if totals.len() > FEW || counts.len() != totals.len() {
        return None;
    }
    let (mut sums, mut over) = ([[0_i128; FEW]; LANES], false);
    let mut fed = |sum: &mut [i128; FEW], (&at, &g): (&u32, &u32)| {
        if let Some(total) = sum.get_mut(index(g)) {
            let unit = units.get(index(at)).copied().unwrap_or(0);
            let (made, overflowed) = total.overflowing_add(i128::from(unit));
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
    for sum in &sums {
        for (total, &part) in totals.iter_mut().zip(sum) {
            let (made, overflowed) = total.overflowing_add(part);
            (*total, over) = (made, over | overflowed);
        }
    }
    counted(counts, groups);
    Some(over)
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
