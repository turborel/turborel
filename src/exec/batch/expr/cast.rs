//! \[DESIGN\] CAST over vectors (RULES.md D362) [ISO/IEC 9075:1992
//! §6.10]: of exact numbers as integers to an exact type, each rescaled
//! as an assignment rescales it, its digits past the new scale truncated,
//! then judged against the type's precision [GR3a, §9.2 GR3k]; of
//! anything else each row's value cast as a row's is (`cast_warned`). A
//! row whose cast would raise an error, or a warning, gives none, and the
//! batch is evaluated a row at a time, which raises it in order.

use super::ints::zeroed;
use super::vector::{Vector, values};
use crate::storage::{Column, Ints};
use crate::value::{DataType, cast_warned, pow10};

/// `x` CAST to `to`, `count` rows of it: none if a row's would raise.
pub(super) fn cast(x: &Vector<'_>, to: &DataType, count: usize) -> Option<Vector<'static>> {
    if let (Some(Column::Ints(ints)), DataType::Exact { precision, scale, .. }) = (x.column(), to) {
        return exact(ints, (*precision, *scale)).map(|made| Vector::Owned(Column::Ints(made)));
    }
    if let Vector::Const(v) = x {
        return match cast_warned(v.clone(), to) {
            Ok((made, None)) => Some(Vector::Const(made)),
            Ok((_, Some(_))) | Err(_) => None,
        };
    }
    let seq = x.seq();
    let mut made = Vec::with_capacity(count);
    for n in 0..count {
        match cast_warned(seq.get(n).clone(), to) {
            Ok((v, None)) => made.push(v),
            Ok((_, Some(_))) | Err(_) => return None,
        }
    }
    Some(values(made))
}

/// [ISO/IEC 9075:1992 §6.10 GR3a, §9.2 GR3k] `ints` as exact numbers of
/// `precision` and `scale`: none if one not null does not fit.
fn exact(ints: &Ints, (precision, scale): (u32, u32)) -> Option<Ints> {
    let limit = pow10(precision)?.unsigned_abs();
    let mut units: Vec<i64> = if scale >= ints.scale {
        let factor = i64::try_from(pow10(scale.checked_sub(ints.scale)?)?).ok()?;
        ints.units.iter().map(|u| u.saturating_mul(factor)).collect()
    } else {
        // Truncated toward zero, as an assignment's quotient is.
        let divisor = i64::try_from(pow10(ints.scale.checked_sub(scale)?)?).ok()?;
        ints.units.iter().map(|u| u.checked_div(divisor).unwrap_or(0)).collect()
    };
    if let Some(nulls) = &ints.nulls {
        zeroed(&mut units, nulls);
    }
    let fits = |u: &i64| u128::from(u.unsigned_abs()) < limit && *u != i64::MIN && *u != i64::MAX;
    if !units.iter().all(fits) {
        return None;
    }
    let made = Ints::new(units, scale, ints.nulls.clone());
    Some(match ints.runs() {
        Some(ends) => made.with_runs(ends.to_vec()),
        None => made,
    })
}
