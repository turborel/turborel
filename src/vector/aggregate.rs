//! \[EXTENSION\] VECTOR's set functions (design document §6.5, §7.1):
//! element-wise VECTOR_SUM and VECTOR_AVG, for centroids, VECTOR_MIN and
//! VECTOR_MAX, and VECTOR_QUANTIZE_PARAMS, an array of the scale and zero
//! point that map the values' range onto INT8's. The design document's
//! `SUM(v)` and `AVG(v)` are the binder's to turn into VECTOR_SUM and
//! VECTOR_AVG for a vector argument.
//!
//! Each accumulates in `f64`, in the order given. A sum and an average
//! are of the wider format, INT8 made FLOAT32 (R6.9); a least or greatest
//! keeps a format all its values share. Values all sparse give a sparse
//! result (R6.10), else a dense one. With no values the result is null
//! [X3.135-1986 §5.8 GR4].

use std::collections::BTreeMap;
use std::sync::Arc;

use super::build::Vector;
use super::functions::FLOAT64_TYPE;
use super::params::shape_of;
use super::{Element, Layout, data_type, mismatch, vector};
use crate::error::{Code, Result, SqlError};
use crate::functions::{Accumulator, AggregateFunction};
use crate::value::{DataType, Value};

/// The INT8 range's steps, 255, that VECTOR_QUANTIZE_PARAMS spreads the
/// values' range over.
const INT8_STEPS: f64 = 255.0;
/// The scale and zero point.
const PARAMS: u32 = 2;

/// A set function of VECTOR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregate {
    /// VECTOR_SUM(v).
    Sum,
    /// VECTOR_AVG(v).
    Avg,
    /// VECTOR_MIN(v).
    Min,
    /// VECTOR_MAX(v).
    Max,
    /// VECTOR_QUANTIZE_PARAMS(v): ARRAY[scale, zero_point].
    QuantizeParams,
}

impl Aggregate {
    /// Every set function of VECTOR.
    pub const ALL: [Aggregate; 5] = [Aggregate::Sum, Aggregate::Avg, Aggregate::Min, Aggregate::Max, Aggregate::QuantizeParams];
}

/// The set functions of VECTOR.
pub fn aggregates() -> Vec<Arc<dyn AggregateFunction>> {
    Aggregate::ALL.into_iter().map(|a| -> Arc<dyn AggregateFunction> { Arc::new(a) }).collect()
}

impl AggregateFunction for Aggregate {
    fn name(&self) -> &'static str {
        match self {
            Aggregate::Sum => "VECTOR_SUM",
            Aggregate::Avg => "VECTOR_AVG",
            Aggregate::Min => "VECTOR_MIN",
            Aggregate::Max => "VECTOR_MAX",
            Aggregate::QuantizeParams => "VECTOR_QUANTIZE_PARAMS",
        }
    }

    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        if arg.as_ref().and_then(shape_of).is_none() {
            return Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {} takes a vector", self.name())));
        }
        Ok(match self {
            Aggregate::QuantizeParams => DataType::array(FLOAT64_TYPE, PARAMS),
            Aggregate::Sum | Aggregate::Avg | Aggregate::Min | Aggregate::Max => data_type(),
        })
    }

    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(Running {
            of: *self,
            dims: 0,
            element: None,
            mixed: false,
            sparse: true,
            count: 0,
            sums: BTreeMap::new(),
            range: (f64::INFINITY, f64::NEG_INFINITY),
        })
    }
}

/// One dimension's running value and the count of vectors storing it.
#[derive(Debug, Clone, Copy)]
struct Slot {
    value: f64,
    stored: u64,
}

/// A group's running state.
struct Running {
    of: Aggregate,
    dims: u32,
    /// The format so far: the first value's, or for a sum the wider one.
    element: Option<Element>,
    /// Whether the values' formats differ.
    mixed: bool,
    /// Whether every value so far was sparse.
    sparse: bool,
    count: u64,
    sums: BTreeMap<u32, Slot>,
    /// The least and greatest element, unstored zeros among them.
    range: (f64, f64),
}

impl Running {
    fn fold(&self, old: f64, new: f64) -> f64 {
        match self.of {
            Aggregate::Sum | Aggregate::Avg => old + new,
            Aggregate::Min => old.min(new),
            Aggregate::Max | Aggregate::QuantizeParams => old.max(new),
        }
    }

    /// The value at a dimension, its absences in sparse values counted as
    /// zeros.
    fn at(&self, slot: Slot) -> f64 {
        if slot.stored >= self.count {
            return slot.value;
        }
        match self.of {
            Aggregate::Sum | Aggregate::Avg => slot.value,
            Aggregate::Min | Aggregate::QuantizeParams => slot.value.min(0.0),
            Aggregate::Max => slot.value.max(0.0),
        }
    }

    fn params(&self) -> Value {
        let (low, high) = self.range;
        let spread = high - low;
        let scale = if spread > 0.0_f64 { spread / INT8_STEPS } else { 1.0_f64 };
        let zero = (f64::from(i8::MIN) - low / scale).round();
        Value::Array(Arc::from(vec![Value::Approx(scale), Value::Approx(zero)]))
    }
}

impl Accumulator for Running {
    fn update(&mut self, v: &Value) -> Result<()> {
        let view = vector(v)?;
        if self.count > 0 && view.dims != self.dims {
            return Err(mismatch(&format!("dimension mismatch: {} and {}", self.dims, view.dims)));
        }
        let summed = matches!(self.of, Aggregate::Sum | Aggregate::Avg | Aggregate::QuantizeParams);
        if summed && matches!(view.element, Element::Binary) {
            return Err(mismatch("BINARY vectors have no sum"));
        }
        self.mixed |= self.element.is_some_and(|e| e != view.element);
        self.element = Some(self.element.map_or(view.element, |e| if e == view.element { e } else { e.wider(view.element) }));
        if view.stored() < view.dims {
            self.range = (self.range.0.min(0.0), self.range.1.max(0.0));
        }
        self.sparse &= matches!(view.layout(), Layout::Sparse);
        self.dims = view.dims;
        self.count = self.count.saturating_add(1);
        for i in 0..view.stored() {
            let (Some(dim), Some(x)) = (view.index(i), view.value(i)) else { continue };
            self.range = (self.range.0.min(x), self.range.1.max(x));
            let fresh = Slot { value: x, stored: 1 };
            let next = self.sums.get(&dim).map_or(fresh, |s| Slot { value: self.fold(s.value, x), stored: s.stored.saturating_add(1) });
            self.sums.insert(dim, next);
        }
        Ok(())
    }

    fn finish(&self) -> Result<Value> {
        let Some(first) = self.element else { return Ok(Value::Null) };
        if matches!(self.of, Aggregate::QuantizeParams) {
            return Ok(self.params());
        }
        let element = match (self.of, self.mixed) {
            (Aggregate::Min | Aggregate::Max, false) => first,
            (Aggregate::Sum | Aggregate::Avg | Aggregate::Min | Aggregate::Max | Aggregate::QuantizeParams, _) => first.wider(first),
        };
        let count = super::float::wide(i64::try_from(self.count).unwrap_or(i64::MAX));
        let mean = matches!(self.of, Aggregate::Avg);
        let value = |s: &Slot| if mean { self.at(*s) / count } else { self.at(*s) };
        if self.sparse {
            return Vector::sparse(element, self.dims, self.sums.iter().map(|(d, s)| (*d, value(s))).collect())?.value();
        }
        Vector::dense(element, (0..self.dims).map(|d| self.sums.get(&d).map_or(0.0_f64, value)).collect())?.value()
    }
}
