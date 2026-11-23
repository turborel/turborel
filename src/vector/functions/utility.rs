//! \[EXTENSION\] VECTOR's utility functions (design document §6.4), and
//! VECTOR_TO_ARRAY and ARRAY_TO_VECTOR, as the engine has arrays (RULES.md
//! D68). Elements are counted from 1.

use std::sync::Arc;

use super::{Arg, FLOAT64_TYPE, INTEGER_TYPE, integer_at, text_at, vector_at};
use crate::error::{Result, SqlState};
use crate::kernels::Portable;
use crate::value::types::MAX_ARRAY_CARDINALITY;
use crate::value::{DataType, Decimal, Value};
use crate::vector::build::{Entries, Vector};
use crate::vector::terms::norm;
use crate::vector::{Element, Layout, VectorRef, data_type, image, mismatch};

/// VECTOR_NORM's norms (§6.4).
const NORM1: &str = "NORM1";
/// The Euclidean norm, the default.
const NORM2: &str = "NORM2";
/// The greatest magnitude.
const NORMINF: &str = "NORMINF";
/// VECTORPROPERTY's properties.
const DIMENSIONS: &str = "DIMENSIONS";
/// The element format, in lower case as SQL Server writes it.
const BASE_TYPE: &str = "BASETYPE";
/// The length of a format's, a layout's or a property's name.
const NAME_LENGTH: u32 = 16;

/// A utility function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Utility {
    /// VECTOR_NORM(v [, 'norm1' | 'norm2' | 'norminf']).
    Norm,
    /// VECTOR_NORMALIZE(v [, norm]), null for a zero vector.
    Normalize,
    /// VECTOR_DIMENSION_COUNT(v).
    DimensionCount,
    /// VECTOR_DIMENSION_FORMAT(v).
    DimensionFormat,
    /// VECTOR_LAYOUT(v).
    Layout,
    /// VECTOR_NNZ(v), its elements not zero.
    Nonzero,
    /// VECTOR_ELEMENT(v, i).
    Element,
    /// VECTOR_SLICE(v, start, len).
    Slice,
    /// VECTOR_CONCAT(a, b).
    Concat,
    /// VECTOR_EQUALS(a, b): the same format, dimensions and bits (R3.12).
    Equals,
    /// VECTORPROPERTY(v, 'Dimensions' | 'BaseType').
    Property,
    /// VECTOR_TO_ARRAY(v).
    ToArray,
    /// ARRAY_TO_VECTOR(array [, format]).
    FromArray,
}

impl Utility {
    /// Every utility.
    pub const ALL: [Utility; 13] = [
        Utility::Norm,
        Utility::Normalize,
        Utility::DimensionCount,
        Utility::DimensionFormat,
        Utility::Layout,
        Utility::Nonzero,
        Utility::Element,
        Utility::Slice,
        Utility::Concat,
        Utility::Equals,
        Utility::Property,
        Utility::ToArray,
        Utility::FromArray,
    ];

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Utility::Norm => "VECTOR_NORM",
            Utility::Normalize => "VECTOR_NORMALIZE",
            Utility::DimensionCount => "VECTOR_DIMENSION_COUNT",
            Utility::DimensionFormat => "VECTOR_DIMENSION_FORMAT",
            Utility::Layout => "VECTOR_LAYOUT",
            Utility::Nonzero => "VECTOR_NNZ",
            Utility::Element => "VECTOR_ELEMENT",
            Utility::Slice => "VECTOR_SLICE",
            Utility::Concat => "VECTOR_CONCAT",
            Utility::Equals => "VECTOR_EQUALS",
            Utility::Property => "VECTORPROPERTY",
            Utility::ToArray => "VECTOR_TO_ARRAY",
            Utility::FromArray => "ARRAY_TO_VECTOR",
        }
    }

    pub(super) fn arity(self) -> (usize, usize) {
        match self {
            Utility::Norm | Utility::Normalize | Utility::FromArray => (1, 2),
            Utility::DimensionCount | Utility::DimensionFormat | Utility::Layout | Utility::Nonzero | Utility::ToArray => (1, 1),
            Utility::Element | Utility::Concat | Utility::Equals | Utility::Property => (2, 2),
            Utility::Slice => (3, 3),
        }
    }

    pub(super) fn arg(self, at: usize) -> Option<Arg> {
        match (self, at) {
            (Utility::FromArray, 0) => Some(Arg::Array),
            (_, 0) | (Utility::Concat | Utility::Equals, 1) => Some(Arg::Vector),
            (Utility::Norm | Utility::Normalize | Utility::Property | Utility::FromArray, 1) => Some(Arg::Text),
            (Utility::Element | Utility::Slice, 1) | (Utility::Slice, 2) => Some(Arg::Integer),
            _ => None,
        }
    }

    pub(super) fn output(self) -> DataType {
        match self {
            Utility::Norm | Utility::Element => FLOAT64_TYPE,
            Utility::Normalize | Utility::Slice | Utility::Concat | Utility::FromArray => data_type(),
            Utility::DimensionCount | Utility::Nonzero => INTEGER_TYPE,
            Utility::DimensionFormat | Utility::Layout | Utility::Property => DataType::varying(NAME_LENGTH),
            Utility::Equals => DataType::Boolean,
            Utility::ToArray => DataType::array(FLOAT64_TYPE, MAX_ARRAY_CARDINALITY),
        }
    }

    pub(super) fn call(self, args: &[Value]) -> Result<Value> {
        if matches!(self, Utility::FromArray) {
            return from_array(args);
        }
        let v = vector_at(args, 0)?;
        match self {
            Utility::Norm => named_norm(v, text_at(args, 1)?).map(Value::Approx),
            Utility::Normalize => normalize(v, text_at(args, 1)?),
            Utility::DimensionCount => Ok(integer(i128::from(v.dims))),
            Utility::DimensionFormat => Ok(Value::Char(v.element.name().into())),
            Utility::Layout => Ok(Value::Char(v.layout().name().into())),
            Utility::Nonzero => Ok(integer(i128::from(v.nonzero()))),
            Utility::Element => element(v, integer_at(args, 1)?.unwrap_or_default()),
            Utility::Slice => slice(v, integer_at(args, 1)?.unwrap_or_default(), integer_at(args, 2)?.unwrap_or_default()),
            Utility::Concat => concat(v, vector_at(args, 1)?),
            Utility::Equals => Ok(Value::Bool(image(args.first().unwrap_or(&Value::Null)) == image(args.get(1).unwrap_or(&Value::Null)))),
            Utility::Property => property(v, text_at(args, 1)?.unwrap_or_default()),
            Utility::ToArray => to_array(v),
            Utility::FromArray => from_array(args),
        }
    }
}

fn integer(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn out_of_bounds(detail: &str) -> crate::error::SqlError {
    mismatch(detail).with_state(SqlState::ARRAY_ELEMENT_ERROR)
}

/// `v`'s norm named `name`, NORM2 if none.
fn named_norm(v: VectorRef<'_>, name: Option<&str>) -> Result<f64> {
    let name = name.unwrap_or(NORM2);
    let stored = || (0..v.stored()).map(|i| v.value(i).unwrap_or_default().abs());
    if name.eq_ignore_ascii_case(NORM2) {
        return Ok(norm(&Portable, v));
    }
    if name.eq_ignore_ascii_case(NORM1) {
        return Ok(stored().sum());
    }
    if name.eq_ignore_ascii_case(NORMINF) {
        return Ok(stored().fold(0.0_f64, f64::max));
    }
    Err(mismatch(&format!("unknown norm {name}: NORM1, NORM2 or NORMINF")))
}

fn normalize(v: VectorRef<'_>, name: Option<&str>) -> Result<Value> {
    let element = match v.element {
        Element::Binary => return Err(mismatch("a BINARY vector cannot be normalized")),
        Element::Int8 => Element::Float32,
        Element::Float32 | Element::Float16 | Element::Float64 => v.element,
    };
    let size = named_norm(v, name)?;
    if size <= 0.0_f64 {
        return Ok(Value::Null);
    }
    let made = Vector::of(v).converted(element)?;
    made.mapped(|x| x / size).value()
}

fn element(v: VectorRef<'_>, at: i64) -> Result<Value> {
    let dim = at.checked_sub(1).and_then(|d| u32::try_from(d).ok());
    dim.and_then(|d| v.at(d))
        .map(Value::Approx)
        .ok_or_else(|| out_of_bounds(&format!("no element {at} in a vector of {} dimensions", v.dims)))
}

fn slice(v: VectorRef<'_>, start: i64, len: i64) -> Result<Value> {
    let first = start.checked_sub(1).and_then(|s| u32::try_from(s).ok());
    let count = u32::try_from(len).ok().filter(|n| *n > 0);
    let (Some(first), Some(count)) = (first, count) else { return Err(out_of_bounds("a slice starts at 1 and is not empty")) };
    let end = first.checked_add(count).filter(|e| *e <= v.dims).ok_or_else(|| out_of_bounds("a slice past the vector's end"))?;
    let made = Vector::of(v);
    match made.entries {
        Entries::Dense(values) => {
            let part = values.get(usize::try_from(first).unwrap_or_default()..usize::try_from(end).unwrap_or_default()).unwrap_or_default();
            Vector::dense(v.element, part.to_vec())?.value()
        }
        Entries::Sparse(pairs) => {
            let kept = pairs.into_iter().filter(|(d, _)| (first..end).contains(d)).map(|(d, x)| (d.saturating_sub(first), x)).collect();
            Vector::sparse(v.element, count, kept)?.value()
        }
    }
}

fn concat(a: VectorRef<'_>, b: VectorRef<'_>) -> Result<Value> {
    if a.element != b.element {
        return Err(mismatch(&format!("format mismatch: {} and {}", a.element.name(), b.element.name())));
    }
    let dims = a.dims.checked_add(b.dims).ok_or_else(|| mismatch("too many dimensions"))?;
    if matches!((a.layout(), b.layout()), (Layout::Dense, Layout::Dense)) {
        let mut values = Vector::of(a).values();
        values.extend(Vector::of(b).values());
        return Vector::dense(a.element, values)?.value();
    }
    let shifted = |v: VectorRef<'_>, by: u32| -> Vec<(u32, f64)> {
        (0..v.dims).filter_map(|d| Some((d.saturating_add(by), v.at(d)?))).filter(|(_, x)| *x != 0.0_f64).collect()
    };
    let mut pairs = shifted(a, 0);
    pairs.extend(shifted(b, a.dims));
    Vector::sparse(a.element, dims, pairs)?.value()
}

fn property(v: VectorRef<'_>, name: &str) -> Result<Value> {
    if name.eq_ignore_ascii_case(DIMENSIONS) {
        return Ok(Value::Char(v.dims.to_string().into()));
    }
    if name.eq_ignore_ascii_case(BASE_TYPE) {
        return Ok(Value::Char(v.element.name().to_ascii_lowercase().into()));
    }
    Err(mismatch(&format!("unknown vector property {name}: Dimensions or BaseType")))
}

fn to_array(v: VectorRef<'_>) -> Result<Value> {
    if v.dims > MAX_ARRAY_CARDINALITY {
        return Err(out_of_bounds(&format!("an array holds at most {MAX_ARRAY_CARDINALITY} elements")));
    }
    let values: Arc<[Value]> = (0..v.dims).map(|d| Value::Approx(v.at(d).unwrap_or_default())).collect();
    Ok(Value::Array(values))
}

fn from_array(args: &[Value]) -> Result<Value> {
    let Some(Value::Array(items)) = args.first() else { return Err(mismatch("ARRAY_TO_VECTOR takes an array")) };
    let element = match text_at(args, 1)? {
        Some(name) => Element::named(name).ok_or_else(|| mismatch(&format!("unknown vector format {name}")))?,
        None => Element::Float32,
    };
    let values = items
        .iter()
        .map(|x| x.as_f64().ok_or_else(|| mismatch("a vector's elements are numbers, never null")))
        .collect::<Result<Vec<_>>>()?;
    Vector::dense(element, values)?.value()
}
