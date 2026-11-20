//! \[DESIGN\] (RULES.md D354) A comparison of a vector of exact numbers of
//! one scale with a constant, folded once into a comparison of their units,
//! as `Decimal::cmp_value` compares the two [X3.135-1986 §5.11 GR4]: what
//! a filter over batches judges (`exec/batch/judge/ints.rs`) and what a
//! column store's scan judges before it decodes the other columns
//! (`engine/columns/judged.rs`, RULES.md D360), so both fold alike.

use std::cmp::Ordering;

use crate::value::{CompOp, Value, pow10};

/// \[DESIGN\] (RULES.md D354) A comparison of integers of a scale with a
/// constant, folded to that scale: of the units `u`, `u op c`; or true of
/// every row not null, or of none, whatever its unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// `u op c`.
    Of(CompOp, i64),
    /// True of every row not null.
    All,
    /// True of none.
    None,
}

impl Bound {
    /// Whether it is true of the unit `u`, not null.
    pub fn holds(self, u: i64) -> bool {
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
pub fn rescaled_bound(scale: u32, op: CompOp, constant: &Value) -> Option<Bound> {
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

/// Whether `op` holds of an ordering, a null or one too large none.
pub fn holds(op: CompOp, ordering: Option<Ordering>) -> bool {
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
