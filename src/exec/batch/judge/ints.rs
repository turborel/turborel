//! \[DESIGN\] RULES.md D31: a conjunct judged over a vector of exact
//! numbers as integers (`storage::Ints`), no value made of each: its
//! constants and the integers each scaled once to the greater of their
//! scales, then compared as integers, as `Decimal::cmp_value` compares
//! them [X3.135-1986 §5.11 GR4]; a null true of no comparison [§5.11
//! GR2], of IS NULL only [§5.15]. A constant not exact, or one too large
//! to scale, is judged of the values instead. Split from judge.rs (RULES.md
//! R-SIZE-2).

use std::cmp::Ordering;

use super::{Judged, Side};
use crate::exec::batch::vector::ints::lane;
use crate::storage::{Batch, Ints};
use crate::value::{CompOp, Value, pow10};

/// The rows of `kept` that `judged` keeps of `ints`, if it is judged of
/// integers.
pub(super) fn kept(judged: &Judged, ints: &Ints, kept: &[usize]) -> Option<Vec<usize>> {
    let unit = |n: usize| (!ints.is_null(n)).then(|| ints.units.get(n).copied()).flatten();
    match judged {
        // [X3.135-1986 §5.15 GR2-GR4] never unknown.
        Judged::IsNull(_, not) => Some(kept.iter().copied().filter(|&n| ints.is_null(n) != *not).collect()),
        Judged::Compare(_, op, constant) => {
            let (factor, c) = scaled(ints.scale, constant)?;
            let truth = |u: i64| holds(*op, i128::from(u).checked_mul(factor).map(|x| x.cmp(&c)));
            Some(kept.iter().copied().filter(|&n| unit(n).is_some_and(truth)).collect())
        }
        // [X3.135-1986 §5.13 GR2, §5.18] an OR of equalities; NOT of it.
        Judged::In(_, list, not) => {
            let scaled_list: Vec<(i128, i128)> =
                list.iter().filter(|v| !v.is_null()).map(|v| scaled(ints.scale, v)).collect::<Option<_>>()?;
            let has_null = list.iter().any(Value::is_null);
            let equal = |u: i64| scaled_list.iter().any(|&(f, c)| i128::from(u).checked_mul(f) == Some(c));
            let keeps = |u: i64| if *not { !has_null && !equal(u) } else { equal(u) };
            Some(kept.iter().copied().filter(|&n| unit(n).is_some_and(keeps)).collect())
        }
        Judged::Like(..) | Judged::Compared(..) => None,
    }
}

/// The rows of `over`, rows of `batch`, of which `a` `op` `b` is true, each
/// side evaluated as integers (`vector/ints.rs`) and both scaled to the
/// greater scale: none if either's integers are not made.
pub(super) fn compared((left, op, right): (&Side, CompOp, &Side), batch: &Batch, over: &[usize]) -> Option<Vec<usize>> {
    let (first, second) = (lane(&left.0, &left.1, batch, Some(over))?, lane(&right.0, &right.1, batch, Some(over))?);
    let common = first.scale.max(second.scale);
    let factors = (pow10(common.checked_sub(first.scale)?)?, pow10(common.checked_sub(second.scale)?)?);
    let unit = |ints: &Ints, n: usize| (!ints.is_null(n)).then(|| ints.units.get(n).copied()).flatten();
    let scaled = |units: i64, factor: i128| i128::from(units).checked_mul(factor);
    let truth = |n: usize| match (unit(&first, n), unit(&second, n)) {
        (Some(one), Some(other)) => holds(op, scaled(one, factors.0).zip(scaled(other, factors.1)).map(|(p, q)| p.cmp(&q))),
        (None, _) | (_, None) => false,
    };
    Some(over.iter().enumerate().filter(|&(n, _)| truth(n)).map(|(_, &r)| r).collect())
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
fn holds(op: CompOp, ordering: Option<Ordering>) -> bool {
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
