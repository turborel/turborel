//! \[EXTENSION\] Vector arithmetic (design document §6.5): element-wise
//! `v1 + v2`, `v1 - v2` and `v1 * v2`, `scalar * v` and `v / scalar`. The
//! result is of the wider format, INT8 made FLOAT32 (R6.9); two sparse
//! vectors give a sparse one, else a dense one (R6.10). A result out of
//! its format's range is an error, never NULL (§6.6). BINARY vectors
//! have no arithmetic.

use super::{Arg, number_at, vector_at};
use crate::error::{Result, SqlState};
use crate::value::Value;
use crate::vector::build::Vector;
use crate::vector::{Element, Layout, VectorRef, mismatch};

/// An arithmetic function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arithmetic {
    /// VECTOR_ADD(a, b), `a + b`.
    Add,
    /// VECTOR_SUBTRACT(a, b), `a - b`.
    Subtract,
    /// VECTOR_MULTIPLY(a, b), `a * b` element by element.
    Multiply,
    /// VECTOR_SCALE(v, s), `s * v`.
    Scale,
    /// VECTOR_DIVIDE(v, s), `v / s`.
    Divide,
}

impl Arithmetic {
    /// Every arithmetic function.
    pub const ALL: [Arithmetic; 5] = [Arithmetic::Add, Arithmetic::Subtract, Arithmetic::Multiply, Arithmetic::Scale, Arithmetic::Divide];

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Arithmetic::Add => "VECTOR_ADD",
            Arithmetic::Subtract => "VECTOR_SUBTRACT",
            Arithmetic::Multiply => "VECTOR_MULTIPLY",
            Arithmetic::Scale => "VECTOR_SCALE",
            Arithmetic::Divide => "VECTOR_DIVIDE",
        }
    }

    pub(super) fn arg(self, at: usize) -> Option<Arg> {
        let scalar = matches!(self, Arithmetic::Scale | Arithmetic::Divide);
        match (at, scalar) {
            (0, _) | (1, false) => Some(Arg::Vector),
            (1, true) => Some(Arg::Number),
            _ => None,
        }
    }

    pub(super) fn call(self, args: &[Value]) -> Result<Value> {
        let a = vector_at(args, 0)?;
        if matches!(a.element, Element::Binary) {
            return Err(mismatch("BINARY vectors have no arithmetic"));
        }
        let element = a.element.wider(a.element);
        match self {
            Arithmetic::Scale | Arithmetic::Divide => {
                let s = number_at(args, 1)?.unwrap_or_default();
                if matches!(self, Arithmetic::Divide) && s == 0.0_f64 {
                    return Err(mismatch("division of a vector by zero").with_state(SqlState::DIVISION_BY_ZERO));
                }
                let by = |x: f64| if matches!(self, Arithmetic::Divide) { x / s } else { x * s };
                Vector::of(a).converted(element)?.mapped(by).value()
            }
            Arithmetic::Add | Arithmetic::Subtract | Arithmetic::Multiply => pairwise(self, a, vector_at(args, 1)?),
        }
    }
}

fn pairwise(op: Arithmetic, a: VectorRef<'_>, b: VectorRef<'_>) -> Result<Value> {
    if a.dims != b.dims {
        return Err(mismatch(&format!("dimension mismatch: {} and {}", a.dims, b.dims)));
    }
    if matches!(b.element, Element::Binary) {
        return Err(mismatch("BINARY vectors have no arithmetic"));
    }
    let element = a.element.wider(b.element);
    let apply = |x: f64, y: f64| match op {
        Arithmetic::Add => x + y,
        Arithmetic::Subtract => x - y,
        Arithmetic::Multiply | Arithmetic::Scale => x * y,
        Arithmetic::Divide => x / y,
    };
    if matches!((a.layout(), b.layout()), (Layout::Sparse, Layout::Sparse)) {
        let mut dims: Vec<u32> = (0..a.stored()).filter_map(|i| a.index(i)).chain((0..b.stored()).filter_map(|i| b.index(i))).collect();
        dims.sort_unstable();
        dims.dedup();
        let pairs = dims.into_iter().map(|d| (d, apply(a.at(d).unwrap_or_default(), b.at(d).unwrap_or_default()))).collect();
        return Vector::sparse(element, a.dims, pairs)?.value();
    }
    let values = (0..a.dims).map(|d| apply(a.at(d).unwrap_or_default(), b.at(d).unwrap_or_default())).collect();
    Vector::dense(element, values)?.value()
}
