//! A decoded constant operand kept by one function expression. Immutable
//! preparation avoids decoding a query polygon for every input row and
//! introduces no cache lock shared by concurrent sessions. Errors remain
//! at evaluation time, including mixed-SRID and malformed-value errors.

use std::sync::Arc;

mod locator;

use super::{Geometry, GeometryFunction, kind};
use crate::error::Result;
use crate::functions::{ScalarFunction, ScalarRef};
use crate::memory::{MemoryBudget, Reservation};
use crate::params::ParameterType;
use crate::value::{DataType, Value};
use locator::Locator;

struct Prepared {
    function: GeometryFunction,
    position: usize,
    value: Value,
    geometry: Geometry,
    locator: Option<Box<Locator>>,
    _held: Reservation,
}

pub(super) fn specialize(function: GeometryFunction, args: &[Option<&Value>], budget: &Arc<MemoryBudget>) -> Option<ScalarRef> {
    let [left, right] = args else { return None };
    for (position, value) in [left, right].into_iter().enumerate() {
        let Some(value) = value else { continue };
        if let Some(prepared) = Prepared::new(function, position, value, budget) {
            return Some(ScalarRef(Arc::new(prepared)));
        }
    }
    None
}

impl Prepared {
    fn new(function: GeometryFunction, position: usize, value: &Value, budget: &Arc<MemoryBudget>) -> Option<Self> {
        let Value::Extension(payload) = value else { return None };
        let base = size_of::<Self>().checked_add(payload.bytes.len())?;
        let mut held = budget.reserve(base, "a prepared geometry operand").ok()?;
        let geometry = Geometry::from_value(value).ok()?;
        held.resize(base.checked_add(geometry.shape.heap_bytes()?)?).ok()?;
        let locator = Locator::new(&geometry.shape, budget);
        Some(Self { function, position, value: value.clone(), geometry, locator, _held: held })
    }

    fn matches(&self, args: &[&Value]) -> bool {
        let (Value::Extension(was), Some(Value::Extension(now))) = (&self.value, args.get(self.position).copied()) else { return false };
        now.kind.name() == kind::NAME && (Arc::ptr_eq(&was.bytes, &now.bytes) || was.bytes == now.bytes)
    }

    fn intersection(&self, other: &Geometry) -> Result<Option<bool>> {
        let Some(locator) = &self.locator else { return Ok(None) };
        let super::Shape::Point(Some(point)) = &other.shape else { return Ok(None) };
        self.geometry.same_reference(other)?;
        if self.geometry.bounds.zip(other.bounds).is_some_and(|(a, b)| !a.intersects(b)) {
            return Ok(Some(false));
        }
        locator.locate(*point).map(|location| Some(location != super::relate::Location::Exterior))
    }
}

impl ScalarFunction for Prepared {
    fn spatial_operation(&self) -> Option<super::SpatialOperation> {
        self.function.spatial_operation()
    }

    fn name(&self) -> &str {
        self.function.name()
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        self.function.return_type(args)
    }

    fn argument_type(&self, position: usize, args: &[Option<DataType>]) -> Option<DataType> {
        self.function.argument_type(position, args)
    }

    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        self.function.parameter_type(position, args)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let [left, right] = args else { return self.function.call(args) };
        self.call_borrowed(&[left, right], None)
    }

    fn call_borrowed(&self, args: &[&Value], temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<Value> {
        let [left, right] = args else { return self.function.call_borrowed(args, temp) };
        if !self.matches(args) {
            return self.function.call_borrowed(args, temp);
        }
        if left.is_null() || right.is_null() {
            return Ok(Value::Null);
        }
        let other = if self.position == 0 { right } else { left };
        let other = Geometry::from_value(other)?;
        let located = self.intersection(&other)?;
        if self.position == 0 {
            self.function.pair_located(&self.geometry, &other, located)
        } else {
            self.function.pair_located(&other, &self.geometry, located)
        }
    }
}
