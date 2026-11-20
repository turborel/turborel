//! \[DESIGN\] Approximate arithmetic over vectors (RULES.md D362): of an
//! approximate operand and a number, approximate [X3.135-1986 §5.9 SR5],
//! each exact operand made its nearest `f64` as a row's evaluation makes
//! it (`Decimal::to_f64`), then added, subtracted, multiplied or divided
//! in `f64` as `value::arith` does. A divisor of 0, or a result not
//! finite, which a row's evaluation raises as an error of the range of
//! approximate numbers [GR5], sets a flag of the batch, of a row not null,
//! and gives none.

use super::ints::{Exact, exact};
use super::vector::{Vector, either};
use crate::storage::{Column, Floats, Mask};
use crate::value::{ArithOp, Decimal, Value};

/// Approximate numbers of a vector, as `f64`s: a column's, one made of a
/// column of exact numbers, or a constant.
pub(super) enum Approx<'v> {
    Column(&'v [f64], Option<&'v Mask>),
    Made(Vec<f64>, Option<&'v Mask>),
    Const(f64),
}

impl Approx<'_> {
    /// Its number of row `n`.
    fn get(&self, n: usize) -> f64 {
        match self {
            Approx::Column(numbers, _) => numbers.get(n).copied().unwrap_or(0.0),
            Approx::Made(numbers, _) => numbers.get(n).copied().unwrap_or(0.0),
            Approx::Const(f) => *f,
        }
    }

    /// Its nulls, if any is.
    fn nulls(&self) -> Option<&Mask> {
        match self {
            Approx::Column(_, nulls) | Approx::Made(_, nulls) => *nulls,
            Approx::Const(_) => None,
        }
    }

    /// Its numbers, if it is not a constant.
    fn numbers(&self) -> Option<&[f64]> {
        match self {
            Approx::Column(numbers, _) => Some(numbers),
            Approx::Made(numbers, _) => Some(numbers),
            Approx::Const(_) => None,
        }
    }
}

/// Whether `v` is approximate: a vector of approximate numbers or an
/// approximate constant.
pub(super) fn is_approximate(v: &Vector<'_>) -> bool {
    matches!(v, Vector::Const(Value::Approx(_)) | Vector::Column(Column::Floats(_)) | Vector::Owned(Column::Floats(_)))
}

/// The numbers of `v` as `f64`s, if it is approximate or exact: an exact
/// one's each made its nearest, as `Value::as_f64` makes it.
pub(super) fn approx<'v>(v: &'v Vector<'_>) -> Option<Approx<'v>> {
    match v {
        Vector::Const(Value::Approx(f)) => Some(Approx::Const(*f)),
        Vector::Column(Column::Floats(f)) | Vector::Owned(Column::Floats(f)) => Some(Approx::Column(&f.numbers, f.nulls.as_ref())),
        Vector::Const(Value::Exact(d)) => Some(Approx::Const(d.to_f64())),
        Vector::Const(_) | Vector::Column(_) | Vector::Owned(_) => match exact(v)? {
            Exact::Column(ints) => {
                let made = ints.units.iter().map(|&u| Decimal::new(i128::from(u), ints.scale).to_f64()).collect();
                Some(Approx::Made(made, ints.nulls.as_ref()))
            }
            Exact::Const(u, scale) => Some(Approx::Const(Decimal::new(i128::from(u), scale).to_f64())),
        },
    }
}

/// `op` of `x` and `y`, `count` rows of each, in `f64`: none if a row
/// not null divides by 0 or gives a number not finite.
pub(super) fn arith(op: ArithOp, (left, right): (&Approx<'_>, &Approx<'_>), count: usize) -> Option<Floats> {
    let apply = move |first: f64, second: f64| {
        let made = match op {
            ArithOp::Add => first + second,
            ArithOp::Sub => first - second,
            ArithOp::Mul => first * second,
            ArithOp::Div => first / second,
        };
        (made, !made.is_finite() || (op == ArithOp::Div && second == 0.0_f64))
    };
    let mut flag = false;
    let mut step = |(first, second): (f64, f64)| {
        let (made, bad) = apply(first, second);
        flag |= bad;
        made
    };
    let numbers: Vec<f64> = match (left.numbers(), right.numbers()) {
        (Some(xs), Some(ys)) => xs.iter().copied().zip(ys.iter().copied()).map(&mut step).collect(),
        (Some(xs), None) => xs.iter().map(|&first| step((first, right.get(0)))).collect(),
        (None, Some(ys)) => ys.iter().map(|&second| step((left.get(0), second))).collect(),
        (None, None) => vec![step((left.get(0), right.get(0))); count],
    };
    let nulls = either(left.nulls(), right.nulls());
    if let Some(nulls) = &nulls
        && flag
    {
        flag = (0..count).any(|n| !nulls.get(n) && apply(left.get(n), right.get(n)).1);
    }
    if flag {
        return None;
    }
    let mut numbers = numbers;
    if let Some(nulls) = &nulls {
        for (n, slot) in numbers.iter_mut().enumerate() {
            if nulls.get(n) {
                *slot = 0.0_f64;
            }
        }
    }
    Some(Floats::new(numbers, nulls))
}

/// [X3.135-1986 §5.9 GR4] Each of `floats` negated.
pub(super) fn negated(floats: &Floats) -> Floats {
    let numbers = floats.numbers.iter().map(|f| -f).collect();
    Floats::new(numbers, floats.nulls.clone())
}
