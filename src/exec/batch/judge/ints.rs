//! \[DESIGN\] RULES.md D31: a conjunct judged over a vector of exact
//! numbers as integers (`storage::Ints`), no value made of each: its
//! constants and the integers each scaled once to the greater of their
//! scales, then compared as integers, as `Decimal::cmp_value` compares them
//! [X3.135-1986 §5.11 GR4]; a null true of no comparison [§5.11 GR2], of IS
//! NULL only [§5.15]. A constant not exact, or one too large to scale, is
//! judged of the values instead.

use super::{Judged, Side};
use crate::exec::batch::vector::ints::{both_runs, lane_of};
use crate::kernels::Kernels;
pub(super) use crate::storage::bound::{Bound, holds, rescaled_bound};
use crate::storage::mask::index;
use crate::storage::sel::rows_of;
use crate::storage::{Batch, Ints};
use crate::value::{CompOp, Decimal, Value, pow10};

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
        Judged::Like(..) | Judged::Compared(..) | Judged::LikeOf(..) | Judged::Any(_) => None,
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

/// The rows of `over`, rows of `batch`, of which `a` `op` `b` is true, each
/// side evaluated as integers (`vector/ints.rs`) and both scaled to the
/// greater scale: none if either's integers are not made. A column is read
/// in place, a constant once, so a side made of neither makes no vector.
pub(super) fn compared((left, op, right): (&Side, CompOp, &Side), batch: &Batch, over: Option<&[u32]>) -> Option<Vec<u32>> {
    let all = over.map_or_else(|| rows_of(0..batch.rows), <[u32]>::to_vec);
    let given = all.as_slice();
    let first = lane_of(&left.0, &left.1, batch, Some(given))?;
    let second = lane_of(&right.0, &right.1, batch, Some(given))?;
    let common = first.scale().max(second.scale());
    let factors = (pow10(common.checked_sub(first.scale())?)?, pow10(common.checked_sub(second.scale())?)?);
    let scaled = |units: i64, factor: i128| i128::from(units).checked_mul(factor);
    let truth = |n: usize| match (first.unit(n), second.unit(n)) {
        (Some(one), Some(other)) => holds(op, scaled(one, factors.0).zip(scaled(other, factors.1)).map(|(p, q)| p.cmp(&q))),
        (None, _) | (_, None) => false,
    };
    // Of sides known as runs, judged once a run of both (RULES.md D31).
    if let Some(ends) = both_runs((&first, &second), given.len()) {
        let mut kept = Vec::with_capacity(given.len());
        let mut start = 0_usize;
        for end in ends {
            if truth(start) {
                kept.extend_from_slice(given.get(start..end).unwrap_or_default());
            }
            start = end;
        }
        return Some(kept);
    }
    Some(given.iter().enumerate().filter(|&(n, _)| truth(n)).map(|(_, &r)| r).collect())
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
