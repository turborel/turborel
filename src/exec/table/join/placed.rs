//! \[DESIGN\] (RULES.md D368) How a join's table places its rows: counted
//! by slot, the counts summed into where each slot's rows end, each row
//! put after those of its slot put before it, in the order they come, so
//! each slot's rows are in the order they were built; then each slot of
//! more than `LINEAR` rows sorted by hash, stably, so the rows of one hash
//! stay in that order.

use super::LINEAR;

/// Turns `ends`, of each slot after a first 0 its count of rows, into
/// where each slot's rows end.
pub(super) fn summed(ends: &mut [usize]) {
    let mut sum = 0_usize;
    for end in ends.iter_mut() {
        sum = sum.saturating_add(*end);
        *end = sum;
    }
}

/// The rows `built` gives, each with its hash, `count` of them, put in
/// the places of their slots, `slot` of each hash, after the rows before
/// them of that slot: the rows, and their hashes. `ends` says where each
/// slot's rows end, after a first 0.
pub(super) fn scattered(
    built: impl Iterator<Item = (usize, u64)>,
    ends: &[usize],
    slot: impl Fn(u64) -> usize,
    count: usize,
) -> (Vec<u32>, Vec<u64>) {
    let (mut rows, mut hashes) = (vec![0_u32; count], vec![0_u64; count]);
    let mut next: Vec<usize> = ends.to_vec();
    for (r, h) in built {
        let Some(at) = next.get_mut(slot(h)) else { continue };
        if let (Some(row), Some(hash)) = (rows.get_mut(*at), hashes.get_mut(*at)) {
            *row = u32::try_from(r).unwrap_or(u32::MAX);
            *hash = h;
        }
        *at = at.saturating_add(1);
    }
    (rows, hashes)
}

/// Sorts by hash, stably, the rows of each slot of more than `LINEAR`,
/// `ends` saying where each slot's rows end, after a first 0.
pub(super) fn sorted(ends: &[usize], rows: &mut [u32], hashes: &mut [u64]) {
    let mut pairs: Vec<(u64, u32)> = Vec::new();
    for (&from, &to) in ends.iter().zip(ends.iter().skip(1)) {
        if to.saturating_sub(from) <= LINEAR {
            continue;
        }
        let (Some(slot_rows), Some(slot_hashes)) = (rows.get_mut(from..to), hashes.get_mut(from..to)) else { continue };
        pairs.clear();
        pairs.extend(slot_hashes.iter().copied().zip(slot_rows.iter().copied()));
        pairs.sort_by_key(|&(h, _)| h);
        for ((row, hash), (h, r)) in slot_rows.iter_mut().zip(slot_hashes.iter_mut()).zip(&pairs) {
            *row = *r;
            *hash = *h;
        }
    }
}
