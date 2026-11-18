//! \[EXTENSION\] Value-level quantization (design document §7.1), the one
//! way into and out of BINARY (R5.5):
//! * `VECTOR_QUANTIZE(v, 'INT8' [, scale, zero_point])`: each element
//!   `round(x / scale) + zero_point`, clamped to -128 to 127. Without a
//!   scale, the vector's own: its greatest magnitude over 127, zero point
//!   0.
//! * `VECTOR_QUANTIZE(v, 'BINARY' [, threshold])`: a bit set where `x` is
//!   above the threshold, 0 by default.
//! * `VECTOR_QUANTIZE(v, 'FLOAT16')`: each element rounded to binary16.
//! * `VECTOR_DEQUANTIZE(v, scale, zero_point)`: FLOAT32 elements
//!   `(q - zero_point) * scale`.

use super::{Arg, number_at, text_at, vector_at};
use crate::error::Result;
use crate::value::Value;
use crate::vector::build::Vector;
use crate::vector::{Element, VectorRef, mismatch};

/// The greatest INT8 magnitude a symmetric scale maps to.
const INT8_REACH: f64 = 127.0;

/// A quantization function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantize {
    /// VECTOR_QUANTIZE(v, format [, ...]).
    Quantize,
    /// VECTOR_DEQUANTIZE(v, scale, zero_point).
    Dequantize,
}

impl Quantize {
    /// Every quantization function.
    pub const ALL: [Quantize; 2] = [Quantize::Quantize, Quantize::Dequantize];

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Quantize::Quantize => "VECTOR_QUANTIZE",
            Quantize::Dequantize => "VECTOR_DEQUANTIZE",
        }
    }

    pub(super) fn arity(self) -> (usize, usize) {
        match self {
            Quantize::Quantize => (2, 4),
            Quantize::Dequantize => (3, 3),
        }
    }

    pub(super) fn arg(self, at: usize) -> Option<Arg> {
        match (self, at) {
            (_, 0) => Some(Arg::Vector),
            (Quantize::Quantize, 1) => Some(Arg::Text),
            (Quantize::Quantize, 2 | 3) | (Quantize::Dequantize, 1 | 2) => Some(Arg::Number),
            _ => None,
        }
    }

    pub(super) fn call(self, args: &[Value]) -> Result<Value> {
        let v = vector_at(args, 0)?;
        match self {
            Quantize::Dequantize => {
                let (scale, zero) = (number_at(args, 1)?.unwrap_or(1.0), number_at(args, 2)?.unwrap_or_default());
                dense(v, Element::Float32, |x| (x - zero) * scale)
            }
            Quantize::Quantize => {
                let name = text_at(args, 1)?.unwrap_or_default();
                let target = Element::named(name).ok_or_else(|| mismatch(&format!("unknown vector format {name}")))?;
                quantize(v, target, number_at(args, 2)?, number_at(args, 3)?)
            }
        }
    }
}

/// `v`'s values through `f`, as a dense vector of `element`.
fn dense(v: VectorRef<'_>, element: Element, f: impl Fn(f64) -> f64) -> Result<Value> {
    let values = (0..v.dims).map(|d| f(v.at(d).unwrap_or_default())).collect();
    Vector::dense(element, values)?.value()
}

fn quantize(v: VectorRef<'_>, target: Element, first: Option<f64>, second: Option<f64>) -> Result<Value> {
    if matches!(v.element, Element::Binary) {
        return Err(mismatch("a BINARY vector is already quantized"));
    }
    match target {
        Element::Int8 => {
            let reach = (0..v.stored()).map(|i| v.value(i).unwrap_or_default().abs()).fold(0.0_f64, f64::max);
            let scale = first.unwrap_or(if reach > 0.0_f64 { reach / INT8_REACH } else { 1.0 });
            if scale <= 0.0_f64 || !scale.is_finite() {
                return Err(mismatch("a quantization scale is a positive number"));
            }
            let zero = second.unwrap_or_default();
            let (low, high) = (f64::from(i8::MIN), f64::from(i8::MAX));
            dense(v, Element::Int8, |x| ((x / scale).round() + zero).round().clamp(low, high))
        }
        Element::Binary => {
            let threshold = first.unwrap_or_default();
            dense(v, Element::Binary, |x| f64::from(u8::from(x > threshold)))
        }
        Element::Float16 => Vector::of(v).converted(Element::Float16)?.value(),
        Element::Float32 | Element::Float64 => Err(mismatch("VECTOR_QUANTIZE makes INT8, BINARY or FLOAT16 vectors")),
    }
}
