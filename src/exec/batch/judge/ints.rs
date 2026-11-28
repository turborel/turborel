//! \[DESIGN\] RULES.md D31: a conjunct judged over a vector of exact
//! numbers as integers (`storage::Ints`), no value made of each: its
//! constants folded once to the integers' scale, then compared as
//! integers, as `Decimal::cmp_value` compares them [X3.135-1986 §5.11
//! GR4], an IN list's kept in order and looked into (`storage::listed`,
//! RULES.md D1295); a null true of no comparison [§5.11 GR2], of IS NULL
//! only [§5.15]. A constant not exact, or one too large to scale, is
//! judged of the values instead.

use super::Judged;
use crate::kernels::Kernels;
use crate::storage::Ints;
pub(super) use crate::storage::bound::{Bound, holds, rescaled_bound};
use crate::storage::listed::Listed;
use crate::storage::mask::index;
use crate::storage::sel::rows_of;
use crate::value::{Decimal, Value};

/// The rows of `over`, or of all, that `judged` keeps of `ints`, if it is
/// judged of integers: a comparison by the kernel `select_i64`, its
/// constant folded to the column's scale once (`rescaled_bound`); of
/// integers known as runs, judged once a run (`Ints::kept_by_runs`).
pub(super) fn kept(judged: &Judged, ints: &Ints, over: Option<&[u32]>, kernels: &dyn Kernels) -> Option<Vec<u32>> {
    // The rows made only when judged one by one, not by the kernel or by
    // runs (RULES.md D1084).
    let rows = || over.map_or_else(|| rows_of(0..ints.units.len()), <[u32]>::to_vec);
    let unit = |n: u32| {
        let n = index(n);
        (!ints.is_null(n)).then(|| ints.units.get(n).copied()).flatten()
    };
    let filtered = |keeps: &dyn Fn(i64) -> bool| {
        ints.kept_by_runs(over, keeps).unwrap_or_else(|| rows().iter().copied().filter(|&n| unit(n).is_some_and(keeps)).collect())
    };
    match judged {
        // [X3.135-1986 §5.15 GR2-GR4] never unknown.
        Judged::IsNull(_, not) => Some(super::kinds::nulls_kept(ints.nulls.as_ref(), (ints.units.len(), over), *not, kernels)),
        Judged::Compare(_, op, constant) => {
            let bound = rescaled_bound(ints.scale, *op, constant)?;
            if ints.runs().is_some() {
                return Some(filtered(&|u| bound.holds(u)));
            }
            let mut out = Vec::new();
            match bound {
                Bound::Of(folded, c) => kernels.select_i64(&ints.units, (folded, c), (ints.nulls.as_ref(), over), &mut out),
                Bound::All => return Some(super::kinds::nulls_kept(ints.nulls.as_ref(), (ints.units.len(), over), true, kernels)),
                Bound::None => {}
            }
            Some(out)
        }
        // [X3.135-1986 §5.13 GR2, §5.18] an OR of equalities; NOT of it:
        // of exact constants, by their units folded to the vector's
        // scale once (RULES.md D1295); of a long list of others by its
        // set, each integer made the number it is.
        Judged::In(_, set, not) => {
            if let Some(listed) = Listed::of(ints.scale, set.values(), *not) {
                return Some(filtered(&|u| listed.holds(u)));
            }
            if !set.hashed() {
                return None;
            }
            let want = super::wanted(*not);
            Some(filtered(&|u: i64| set.truth(&Value::Exact(Decimal::new(i128::from(u), ints.scale))) == want))
        }
        Judged::Like(..) | Judged::Truth(..) | Judged::LikeOf(..) | Judged::Any(_) | Judged::Unpadded(..) => None,
    }
}

#[cfg(test)]
mod tests;
