//! \[DESIGN\] RULES.md D31: a conjunct judged over a vector of exact
//! numbers as integers (`storage::Ints`), no value made of each: its
//! constants and the integers each scaled once to the greater of their
//! scales, then compared as integers, as `Decimal::cmp_value` compares them
//! [X3.135-1986 §5.11 GR4]; a null true of no comparison [§5.11 GR2], of IS
//! NULL only [§5.15]. A constant not exact, or one too large to scale, is
//! judged of the values instead.

use super::Judged;
use crate::kernels::Kernels;
use crate::storage::Ints;
pub(super) use crate::storage::bound::{Bound, holds, rescaled_bound};
use crate::storage::mask::index;
use crate::storage::sel::rows_of;
use crate::value::{Decimal, Value, pow10};

/// The rows of `over`, or of all, that `judged` keeps of `ints`, if it is
/// judged of integers: a comparison by the kernel `select_i64`, its
/// constant folded to the column's scale once (`rescaled_bound`); of
/// integers known as runs, judged once a run (`by_runs`).
pub(super) fn kept(judged: &Judged, ints: &Ints, over: Option<&[u32]>, kernels: &dyn Kernels) -> Option<Vec<u32>> {
    let rows = over.map_or_else(|| rows_of(0..ints.units.len()), <[u32]>::to_vec);
    let unit = |n: u32| {
        let n = index(n);
        (!ints.is_null(n)).then(|| ints.units.get(n).copied()).flatten()
    };
    let filtered = |keeps: &dyn Fn(i64) -> bool| {
        by_runs(ints, &rows, keeps).unwrap_or_else(|| rows.iter().copied().filter(|&n| unit(n).is_some_and(keeps)).collect())
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
        // a long list's by its set, each integer made the number it is.
        Judged::In(_, set, not) => {
            if set.hashed() {
                let want = super::wanted(*not);
                return Some(filtered(&|u: i64| set.truth(&Value::Exact(Decimal::new(i128::from(u), ints.scale))) == want));
            }
            let list = set.values();
            let scaled_list: Vec<(i128, i128)> =
                list.iter().filter(|v| !v.is_null()).map(|v| scaled(ints.scale, v)).collect::<Option<_>>()?;
            let has_null = list.iter().any(Value::is_null);
            let equal = |u: i64| scaled_list.iter().any(|&(f, c)| i128::from(u).checked_mul(f) == Some(c));
            Some(filtered(&|u| if *not { !has_null && !equal(u) } else { equal(u) }))
        }
        Judged::Like(..) | Judged::Truth(..) | Judged::LikeOf(..) | Judged::Any(_) => None,
    }
}

/// \[DESIGN\] (RULES.md D31) The rows of `kept` of whose unit `keeps` is
/// true, judged once a run of one unit, its rows kept or not together, a
/// null never, if `ints` are known as runs (`Ints::runs_of`).
fn by_runs(ints: &Ints, kept: &[u32], keeps: &dyn Fn(i64) -> bool) -> Option<Vec<u32>> {
    let mut out = Vec::with_capacity(kept.len());
    for (unit, rows) in ints.runs_of(kept)? {
        match (keeps(unit), &ints.nulls) {
            (false, _) => {}
            (true, None) => out.extend_from_slice(rows),
            (true, Some(_)) => out.extend(rows.iter().filter(|&&n| !ints.is_null(index(n)))),
        }
    }
    Some(out)
}

/// The factor each integer of `scale` is multiplied by, and `constant` at
/// the greater of their scales: none if it is not an exact number, or
/// either does not fit 128 bits there.
fn scaled(scale: u32, constant: &Value) -> Option<(i128, i128)> {
    let Value::Exact(d) = constant else { return None };
    let common = scale.max(d.scale);
    let factor = pow10(common.checked_sub(scale)?)?;
    let c = d.units.checked_mul(pow10(common.checked_sub(d.scale)?)?)?;
    Some((factor, c))
}

#[cfg(test)]
mod tests;
