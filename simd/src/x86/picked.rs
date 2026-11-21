//! \[DESIGN\] Sums and extremes of a selection (RULES.md D570): its rows
//! gathered a block at a time onto the stack by the vector gathers, each
//! block then summed or compared by the bodies of all rows, so a
//! grouping's rows of a group are added as a scan's are. A selection of
//! fewer rows than `PICKS_LEAST`, whose gathers would cost more than they
//! save, and one with a row past the end, which the portable extremes
//! leave out, run the portable body.

use turborel::kernels::portable;
use turborel::storage::Mask;

use super::sum;

/// The fewest rows of a selection gathered: below them, the portable
/// body is as fast.
pub(super) const PICKS_LEAST: usize = 64;

/// Rows gathered at a time, onto the stack.
const BLOCK: usize = 256;

/// The vector set a selection is gathered and reduced on.
#[derive(Clone, Copy)]
pub(super) enum Wide {
    Avx2,
    Avx512,
}

impl Wide {
    /// The values at `sel` into `slots`.
    fn gather(self, values: &[i64], picks: (&[u32], i32), slots: &mut [i64]) {
        match self {
            Wide::Avx2 => sum::picked_avx2(values, picks, slots),
            Wide::Avx512 => sum::picked_avx512(values, picks, slots),
        }
    }

    /// The sum of `values`.
    fn sum(self, values: &[i64]) -> i128 {
        match self {
            Wide::Avx2 => sum::sum_avx2(values),
            Wide::Avx512 => sum::sum_avx512(values),
        }
    }

    /// The least and greatest of `values`, not null.
    fn extremes(self, values: &[i64], nulls: Option<&Mask>) -> Option<(i64, i64)> {
        match self {
            Wide::Avx2 => sum::min_max_avx2(values, nulls),
            Wide::Avx512 => sum::min_max_avx512(values, nulls),
        }
    }
}

/// `Kernels::sum_i64` of the rows `sel`, on `wide`: a row past the end
/// adds 0, as the portable body's.
pub(super) fn sum_at(wide: Wide, values: &[i64], sel: &[u32]) -> i128 {
    let Some(len) = sum::indexable(values).filter(|_| sel.len() >= PICKS_LEAST) else {
        return portable::sum::sum_i64(values, Some(sel));
    };
    let mut block = [0_i64; BLOCK];
    let mut total = 0_i128;
    for chunk in sel.chunks(BLOCK) {
        let slots = block.get_mut(..chunk.len()).unwrap_or_default();
        wide.gather(values, (chunk, len), slots);
        total = total.wrapping_add(wide.sum(slots));
    }
    total
}

/// `Kernels::min_max_i64` of the rows `sel`, on `wide`, a null row's
/// value left out by its bit gathered with it.
pub(super) fn min_max_at(wide: Wide, values: &[i64], (nulls, sel): (Option<&Mask>, &[u32])) -> Option<(i64, i64)> {
    let within = sum::indexable(values).filter(|&len| sel.iter().fold(0, |m, &r| m.max(r)) < len.unsigned_abs());
    let Some(len) = within.filter(|_| sel.len() >= PICKS_LEAST) else {
        return portable::sum::min_max_i64(values, (nulls, Some(sel)));
    };
    let nulls = nulls.filter(|m| m.any());
    let mut block = [0_i64; BLOCK];
    let mut best: Option<(i64, i64)> = None;
    for chunk in sel.chunks(BLOCK) {
        let slots = block.get_mut(..chunk.len()).unwrap_or_default();
        wide.gather(values, (chunk, len), slots);
        let held = nulls.and_then(|m| m.gathered(chunk));
        let found = wide.extremes(slots, held.as_ref());
        best = match (best, found) {
            (Some((lo, hi)), Some((l, h))) => Some((lo.min(l), hi.max(h))),
            (None, found) | (found, None) => found,
        };
    }
    best
}
