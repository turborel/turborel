//! \[DESIGN\] RULES.md D31: a conjunct judged over a vector of exact
//! numbers as integers (`storage::Ints`), no value made of each: its
//! constants and the integers each scaled once to the greater of their
//! scales, then compared as integers, as `Decimal::cmp_value` compares them
//! [X3.135-1986 §5.11 GR4]; a null true of no comparison [§5.11 GR2], of IS
//! NULL only [§5.15]. A constant not exact, or one too large to scale, is
//! judged of the values instead.

use std::cmp::Ordering;

use super::Judged;
use crate::kernels::Kernels;
use crate::storage::Ints;
use crate::storage::mask::index;
use crate::storage::sel::rows_of;
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
        Judged::Like(..) | Judged::Truth(..) | Judged::LikeOf(..) | Judged::Any(_) => None,
    }
}

/// \[DESIGN\] (RULES.md D354) A comparison of integers of a scale with a
/// constant, folded to that scale: of the units `u`, `u op c`; or true of
/// every row not null, or of none, whatever its unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Bound {
    /// `u op c`.
    Of(CompOp, i64),
    /// True of every row not null.
    All,
    /// True of none.
    None,
}

impl Bound {
    /// Whether it is true of the unit `u`, not null.
    fn holds(self, u: i64) -> bool {
        match self {
            Bound::Of(op, c) => holds(op, Some(u.cmp(&c))),
            Bound::All => true,
            Bound::None => false,
        }
    }
}

/// \[DESIGN\] (RULES.md D354) `v op constant` of an exact number `v` of
/// `scale`, folded once for a batch into a comparison of its units, as
/// `Decimal::cmp_value` compares the two [X3.135-1986 §5.11 GR4]: a
/// constant of a smaller or equal scale scaled up to the column's; one of
/// a greater scale, by `k = 10^(d - s)` and its units `c`, `<` as
/// `u < ceil(c/k)`, `<=` as `u <= floor(c/k)`, `>` as `u > floor(c/k)`,
/// `>=` as `u >= ceil(c/k)`, `=` true of none if `k` does not divide `c`
/// else `u = c/k`, `<>` of every row not null if it does not, else
/// `u <> c/k`; a bound past an `i64` true of every row or of none by its
/// sign. None if the constant is not an exact number, or too large to
/// scale.
pub(super) fn rescaled_bound(scale: u32, op: CompOp, constant: &Value) -> Option<Bound> {
    let Value::Exact(d) = constant else { return None };
    if d.scale <= scale {
        let c = d.units.checked_mul(pow10(scale.checked_sub(d.scale)?)?)?;
        return Some(bounded(op, c));
    }
    let k = pow10(d.scale.checked_sub(scale)?)?;
    let (floor, rem) = (d.units.checked_div_euclid(k)?, d.units.checked_rem_euclid(k)?);
    let ceil = if rem == 0 { floor } else { floor.checked_add(1)? };
    Some(match (op, rem == 0) {
        (CompOp::Lt | CompOp::Ge, _) => bounded(op, ceil),
        (CompOp::Le | CompOp::Gt, _) | (CompOp::Eq | CompOp::Ne, true) => bounded(op, floor),
        (CompOp::Eq, false) => Bound::None,
        (CompOp::Ne, false) => Bound::All,
    })
}

/// `u op c` of units `u` and a bound `c` in 128 bits: folded by its sign
/// if it is past an `i64`.
fn bounded(op: CompOp, c: i128) -> Bound {
    if let Ok(c) = i64::try_from(c) {
        return Bound::Of(op, c);
    }
    // Every unit is below a bound past the greatest, above one past the least.
    let below = c > 0;
    match op {
        CompOp::Lt | CompOp::Le => {
            if below {
                Bound::All
            } else {
                Bound::None
            }
        }
        CompOp::Gt | CompOp::Ge => {
            if below {
                Bound::None
            } else {
                Bound::All
            }
        }
        CompOp::Eq => Bound::None,
        CompOp::Ne => Bound::All,
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

/// Whether `op` holds of an ordering, a null or one too large none.
pub(super) fn holds(op: CompOp, ordering: Option<Ordering>) -> bool {
    let Some(o) = ordering else { return false };
    match op {
        CompOp::Eq => o == Ordering::Equal,
        CompOp::Ne => o != Ordering::Equal,
        CompOp::Lt => o == Ordering::Less,
        CompOp::Le => o != Ordering::Greater,
        CompOp::Gt => o == Ordering::Greater,
        CompOp::Ge => o != Ordering::Less,
    }
}

#[cfg(test)]
mod tests;
