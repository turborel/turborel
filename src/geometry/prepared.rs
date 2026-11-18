//! A decoded constant operand kept by one function expression. Immutable
//! preparation avoids decoding a query polygon for every input row and
//! introduces no cache lock shared by concurrent sessions. Errors remain
//! at evaluation time, including mixed-SRID and malformed-value errors.
//! A polygon's cell certificates (`cells.rs`) are built once, the first
//! time the operand has located enough points, or its interval trees
//! tested enough edges, to repay them, so an operand prepared for a few
//! rows of a join never pays for them; the
//! hierarchy of its edges (`tree.rs`), the first time a point in a
//! boundary cell needs the edges near it, or once a few distances have
//! repaid it; a point's distance then searches it for the nearest edge.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

mod cells;
mod locator;
mod rectangle;
mod shapes;
mod tree;

use super::{Geometry, GeometryFunction, kind};
use crate::error::Result;
use crate::functions::{ScalarFunction, ScalarRef};
use crate::memory::{MemoryBudget, Reservation};
use crate::params::ParameterType;
use crate::value::{DataType, Value};
use cells::Cells;
use locator::Locator;
use shapes::FiledEdges;
use tree::EdgeTree;

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
    /// Or once the interval trees' tests, estimated from those of the
    /// first points located, reach `visits_after`.
    visits_after: usize,
    visited: AtomicUsize,
    /// Built at the `filed_after`th line or polygon tested; never if that
    /// is `usize::MAX`.
    filed: OnceLock<Option<Box<FiledEdges>>>,
    filed_after: usize,
    probed: AtomicUsize,
    /// The polygon's edges, and their hierarchy once a point needs it.
    edges: usize,
    tree: OnceLock<Option<Box<EdgeTree>>>,
    /// Point distances measured before the hierarchy is searched.
    measured: AtomicUsize,
    /// The budget certificates, filed edges and the hierarchy would reserve
    /// from, kept only for an operand that may build them.
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
/// \[DESIGN\] Edges the interval trees test, per edge, before the
/// certificates are built however few points were located: building a
/// comb's certificates, whose teeth span every row, costs about as much
/// as 1,400 to 2,200 such tests per edge (`bench/geometry.md`).
const CELL_VISITS_PER_EDGE: usize = 2_048;
/// The first points located whose tests are counted; later points are
/// assumed to test as many, so they pay nothing for the count.
const COUNTED_POINTS: usize = 64;
/// \[DESIGN\] Lines and polygons tested before a polygon's edges are
/// filed: filing costs about one unprepared test, which files the larger
/// side's edges anyway.
const FILED_AFTER_USES: usize = 1;
/// \[DESIGN\] Point distances measured by a scan before the edges'
/// hierarchy is searched instead: building it costs about as much as
/// twelve to sixteen scans of 4,100 to 32,772 edges.
const MEASURED_AFTER_USES: usize = 16;

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
        let edges = cells::edges(&geometry.shape).unwrap_or(0);
        let cells_after = Some(edges)
            .filter(|&edges| valid && edges >= cells::MIN_EDGES)
            .and_then(|edges| edges.checked_mul(CELL_USES_PER_EDGE)?.checked_add(MIN_CELL_USES))
            .unwrap_or(usize::MAX);
        let visits_after = if cells_after == usize::MAX { usize::MAX } else { edges.saturating_mul(CELL_VISITS_PER_EDGE) };
        let rectangle = rectangle::recognizes(&geometry.shape);
        let budget = (cells_after < usize::MAX).then(|| Arc::clone(budget));
        let filed_after = if budget.is_some() { FILED_AFTER_USES } else { usize::MAX };
        let (cells, located, filed, probed) = (OnceLock::new(), AtomicUsize::new(0), OnceLock::new(), AtomicUsize::new(0));
        Some(Self {
            function,
            position,
            value: value.clone(),
            geometry,
            locator,
            cells,
            cells_after,
            located,
            visits_after,
            visited: AtomicUsize::new(0),
            filed,
            filed_after,
            probed,
            edges,
            tree: OnceLock::new(),
            measured: AtomicUsize::new(0),
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
        match &other.shape {
            super::Shape::Point(Some(point)) => self.point_intersection(*point, other),
            super::Shape::Point(None) => Ok(None),
            super::Shape::Line(_) | super::Shape::Polygon(_) | super::Shape::Multi(_, _) => self.shape_intersection(other),
        }
    }

    fn point_intersection(&self, point: super::shape::Point, other: &Geometry) -> Result<Option<bool>> {
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
        // A cell decides the point exactly, by its label or the edges near
        // the point; a cell several rings touch leaves it to the locator.
        if let Some(location) = self.cells().and_then(|cells| cells.decide(point, || self.tree())) {
            return Ok(Some(location != super::crossing::Location::Exterior));
        }
        let Some(locator) = &self.locator else { return Ok(None) };
        self.locate_by(locator, point).map(|location| Some(location != super::crossing::Location::Exterior))
    }

    /// A line or polygon tested against a polygon with filed edges probes
    /// only the edges filed near its own; others take the general path.
    fn shape_intersection(&self, other: &Geometry) -> Result<Option<bool>> {
        if self.filed_after == usize::MAX {
            return Ok(None);
        }
        self.geometry.same_reference(other)?;
        if self.geometry.bounds.zip(other.bounds).is_none_or(|(a, b)| !a.intersects(b)) {
            return Ok(None);
        }
        let Some(filed) = self.filed() else { return Ok(None) };
        let locate = |point| self.location(point);
        Ok(filed.meets(&self.geometry.shape, &other.shape, &locate))
    }

    /// A point's separation from the polygon, for a distance whose point
    /// the polygon does not hold, by a nearest-edge search once enough
    /// distances have repaid the hierarchy: the scan's minimum, to the bit.
    fn separation(&self, other: &Geometry, located: Option<bool>) -> Option<f64> {
        let (GeometryFunction::Distance, super::Shape::Point(Some(point))) = (self.function, &other.shape) else { return None };
        if located == Some(true) || self.cells_after == usize::MAX {
            return None;
        }
        if self.tree.get().is_none() && self.measured.fetch_add(1, Ordering::Relaxed) < MEASURED_AFTER_USES {
            return None;
        }
        self.tree()?.nearest(*point)
    }

    /// Where `point` lies in the prepared polygon, exactly: by its cell,
    /// its interval tree or its rings.
    fn location(&self, point: super::shape::Point) -> super::crossing::Location {
        self.cells().and_then(|cells| cells.decide(point, || self.tree())).unwrap_or_else(|| {
            self.locator
                .as_ref()
                .and_then(|locator| self.locate_by(locator, point).ok())
                .unwrap_or_else(|| super::crossing::locate(point, &self.geometry.shape))
        })
    }
}

impl Prepared {
    /// The polygon's cell certificates, once it has located enough points,
    /// or its interval trees tested enough edges, to repay building them.
    fn cells(&self) -> Option<&Cells> {
        if let Some(cells) = self.cells.get() {
            return cells.as_deref();
        }
        let located = self.located.fetch_add(1, Ordering::Relaxed);
        if located < self.cells_after && !self.repaid(located) {
            return None;
        }
        let build = || Cells::new(&self.geometry.shape, self.geometry.bounds?, self.budget.as_ref()?);
        self.cells.get_or_init(build).as_deref()
    }

    /// Whether the interval trees' tests for `located` points, estimated
    /// from those of the first points counted, reach the certificates' cost.
    fn repaid(&self, located: usize) -> bool {
        let counted = located.clamp(1, COUNTED_POINTS);
        let tests = self.visited.load(Ordering::Relaxed).saturating_mul(located).checked_div(counted);
        tests.is_some_and(|tests| tests >= self.visits_after)
    }

    /// Where `point` lies by the interval trees, counting the edges they
    /// test for the first points located while certificates may follow.
    fn locate_by(&self, locator: &Locator, point: super::shape::Point) -> Result<super::crossing::Location> {
        let counting = self.cells_after < usize::MAX && self.located.load(Ordering::Relaxed) <= COUNTED_POINTS;
        if !counting || self.cells.get().is_some() {
            return locator.locate(point, &mut ());
        }
        let mut tested = 0_usize;
        let location = locator.locate(point, &mut tested);
        self.visited.fetch_add(tested, Ordering::Relaxed);
        location
    }

    /// The hierarchy of the polygon's edges, built the first time it is
    /// asked for.
    fn tree(&self) -> Option<&EdgeTree> {
        let build = || EdgeTree::new(&self.geometry.shape, self.edges, self.budget.as_ref()?);
        self.tree.get_or_init(build).as_deref()
    }

    /// The polygon's filed edges, once it has tested enough lines and
    /// polygons to repay filing them.
    fn filed(&self) -> Option<&FiledEdges> {
        if let Some(filed) = self.filed.get() {
            return filed.as_deref();
        }
        if self.probed.fetch_add(1, Ordering::Relaxed) < self.filed_after {
            return None;
        }
        let build = || FiledEdges::new(&self.geometry.shape, self.geometry.bounds?, self.budget.as_ref()?);
        self.filed.get_or_init(build).as_deref()
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
        let measured = self.separation(&other, located);
        if self.position == 0 {
            self.function.pair_located(&self.geometry, &other, located, measured)
        } else {
            self.function.pair_located(&other, &self.geometry, located, measured)
        }
    }
}
