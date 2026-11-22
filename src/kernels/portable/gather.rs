//! \[DESIGN\] Gathers (RULES.md D354): the values of a vector at the rows
//! of a selection, in order, into one of their own.

use crate::storage::mask::index;

/// Appends to `out` the values of `values` at `sel`, in order; 0 for a row
/// past the end.
#[inline]
pub fn gather_i64(values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
    out.extend(sel.iter().map(|&r| values.get(index(r)).copied().unwrap_or(0)));
}
