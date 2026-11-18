//! A decoded constant operand kept by one function expression. Immutable
//! preparation avoids decoding a query polygon for every input row and
//! introduces no cache lock shared by concurrent sessions. Errors remain
//! at evaluation time, including mixed-SRID and malformed-value errors.
//! A polygon's cell certificates (`cells.rs`) are built once, the first
//! time the operand has located enough points to repay them, so an
//! operand prepared for a few rows of a join never pays for them.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

mod cells;
mod locator;
mod rectangle;

use super::{Geometry, GeometryFunction, kind};
use crate::error::Result;
use crate::functions::{ScalarFunction, ScalarRef};
use crate::memory::{MemoryBudget, Reservation};
use crate::params::ParameterType;
use crate::value::{DataType, Value};
use cells::Cells;
use locator::Locator;

struct Prepared {
    function: GeometryFunction,
    position: usize,
    value: Value,
    geometry: Geometry,
    locator: Option<Box<Locator>>,
    /// Built at the `cells_after`th point located; never if that is
    /// `usize::MAX`.
    cells: OnceLock<Option<Box<Cells>>>,
    cells_after: usize,
    located: AtomicUsize,
    /// The budget certificates would reserve from, kept only for an
    /// operand that may build them.
    budget: Option<Arc<MemoryBudget>>,
    rectangle: bool,
    _held: Reservation,
}

/// \[DESIGN\] Points located before a polygon's cell certificates are
/// built: a grid's cells cost about as much to build as this many lookups
/// save, per edge.
const CELL_USES_PER_EDGE: usize = 8;
/// The fewest points located before building them.
const MIN_CELL_USES: usize = 256;

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
        // The locator's ring parity is the engine's location for a valid
        // polygon only, so another polygon is left to the engine, as are
        // its cells, labelled by the same parity.
        let valid = crate::spatial::planar::is_valid(&super::engine::of_shape(&geometry.shape)).unwrap_or(false);
        let locator = if valid { Locator::new(&geometry.shape, budget) } else { None };
        let cells_after = cells::edges(&geometry.shape)
            .filter(|&edges| valid && edges >= cells::MIN_EDGES)
            .and_then(|edges| edges.checked_mul(CELL_USES_PER_EDGE)?.checked_add(MIN_CELL_USES))
            .unwrap_or(usize::MAX);
        let rectangle = rectangle::recognizes(&geometry.shape);
        let budget = (cells_after < usize::MAX).then(|| Arc::clone(budget));
        let (cells, located) = (OnceLock::new(), AtomicUsize::new(0));
        Some(Self {
            function,
            position,
            value: value.clone(),
            geometry,
            locator,
            cells,
            cells_after,
            located,
            budget,
            rectangle,
            _held: held,
        })
    }

    fn matches(&self, args: &[&Value]) -> bool {
        let (Value::Extension(was), Some(Value::Extension(now))) = (&self.value, args.get(self.position).copied()) else { return false };
        kind::geometry_kind(now.kind).is_some() && (Arc::ptr_eq(&was.bytes, &now.bytes) || was.bytes == now.bytes)
    }

    fn intersection(&self, other: &Geometry) -> Result<Option<bool>> {
        let super::Shape::Point(Some(point)) = &other.shape else { return Ok(None) };
        if self.rectangle {
            self.geometry.same_reference(other)?;
            return Ok(self.geometry.bounds.zip(other.bounds).map(|(a, b)| a.intersects(b)));
        }
        if self.locator.is_none() && self.cells_after == usize::MAX {
            return Ok(None);
        }
        self.geometry.same_reference(other)?;
        if self.geometry.bounds.zip(other.bounds).is_some_and(|(a, b)| !a.intersects(b)) {
            return Ok(Some(false));
        }
        // A labelled cell decides the point exactly; a boundary cell
        // leaves it to the exact edge tests.
        if let Some(decided) = self.cells().and_then(|cells| cells.decide(*point)) {
            return Ok(Some(decided));
        }
        let Some(locator) = &self.locator else { return Ok(None) };
        locator.locate(*point).map(|location| Some(location != super::crossing::Location::Exterior))
    }
}

impl Prepared {
    /// The polygon's cell certificates, once it has located enough points
    /// to repay building them.
    fn cells(&self) -> Option<&Cells> {
        if let Some(cells) = self.cells.get() {
            return cells.as_deref();
        }
        if self.located.fetch_add(1, Ordering::Relaxed) < self.cells_after {
            return None;
        }
        let build = || Cells::new(&self.geometry.shape, self.geometry.bounds?, self.budget.as_ref()?);
        self.cells.get_or_init(build).as_deref()
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
