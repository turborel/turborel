//! \[EXTENSION\] Finite XY geometry values and their interchange formats.
//! WKT and WKB follow the seven basic types of OGC 06-103r4 §§7–8.
//! Ring structure is checked when decoded; polygon topology must be
//! valid for spatial relationships. No coordinate transformation occurs.

mod collections;
mod conversions;
mod crossing;
mod distance;
pub mod engine;
pub(crate) use distance::distance_domain;
mod functions;
mod kind;
pub mod methods;
pub mod model;
mod mutators;
mod observers;
mod ordering;
mod planar;
mod prepared;
mod procedures;
mod reference;
pub(crate) mod registered;
mod representations;
mod robust;
pub mod routines;
pub mod sfa;
mod shape;
mod solids;
pub mod support;
mod tin_element;
mod tins;
mod wkb;
mod wkt;

pub use collections::{COLLECTION_FUNCTIONS, CollectionFunction, MAX_GEOMETRY_ARRAY_ELEMENTS, element_kind, elements};
pub use functions::{FUNCTIONS, GeometryFunction};
pub(crate) use functions::{functions, unit_scale};
pub use kind::{GEOMETRY, Geometries, GeometryType, NAME, TYPES, data_type, data_type_of, kind_of, kinds, most_specific, of_data_type};
pub use ordering::{EQUALITY, ORDERING_EQUALS, OrderingEquals};
pub use procedures::{CatalogProcedure, procedures};
pub use reference::data_type_with_srid;
pub(crate) use reference::fixed_srid;
pub use representations::MAX_GEOMETRY_AS_GML;
use shape::Shape;
pub use solids::{BRepSolid, SOLID_FUNCTIONS, SolidFunction, TinFromTable, TinTable, table_functions};
pub use tin_element::kind as tin_element_kind;

use crate::error::{Code, Result, SqlError, SqlState};
use crate::spatial::{Bounds, DIMENSIONS};
use crate::value::{DataType, Value};

/// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.7] The method
/// ST_IsEmpty() of an ST_TINElement value, if `t` is its type, which a function of the name, the geometry types', does not take.
pub fn tin_element_is_empty(t: &DataType) -> Option<std::sync::Arc<dyn crate::functions::ScalarFunction>> {
    let DataType::Extension(e) = t else { return None };
    (e.kind.name() == tin_element::ST_TINELEMENT)
        .then(|| -> std::sync::Arc<dyn crate::functions::ScalarFunction> { std::sync::Arc::new(tins::TinRoutine::IsEmpty) })
}

/// Maximum nesting, including the outer geometry (RULES.md R-CF-6).
pub const MAX_DEPTH: usize = 16;
/// Maximum WKT, WKB or stored payload length: 16 MiB (RULES.md D85).
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
/// Maximum coordinate and component count per value (RULES.md D85).
pub const MAX_PARTS: usize = 100_000;
/// SRID used by CAST and the one-argument constructors; no CRS is implied.
pub const UNSPECIFIED_SRID: i32 = 0;

/// A spatial function's semantics, usable only with the planner's SRID
/// and runtime-error safeguards (D85); this is not a total Boolean predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpatialOperation {
    /// Closed XY intersection: INTEGER 1/0, NULL for an empty operand and
    /// NULL propagation, and an error for unequal SRIDs, including empty
    /// operands, as [`Geometry::intersects`].
    Intersects,
    /// Minimum Cartesian separation, with precisely the results, envelope
    /// lower bound and errors of [`Geometry::distance`]. NULL propagates;
    /// empty sets return NULL. The SQL result is DOUBLE PRECISION.
    Distance,
    /// A relationship true only of operands whose point sets meet, so of
    /// none that is empty: ST_Touches, ST_Crosses, ST_Within, ST_Contains
    /// and ST_Overlaps, INTEGER 1, 0 or NULL [ISO/IEC 13249-3:2016 §5.1].
    /// An index finds its rows as it finds those of `Intersects`.
    Meets,
    /// ST_Equals, true also of two empty sets [§5.1.43], so an index
    /// finds every row for an empty operand.
    Equals,
    /// INTEGER 1 for an empty geometry and 0 otherwise, as
    /// [`Geometry::is_empty`]; a NULL argument returns NULL.
    IsEmpty,
    /// ST_Relate, INTEGER 1 if the operands' intersection matrix matches
    /// its third argument's pattern [ISO/IEC 13249-3:2016 §5.1.45]. Only a
    /// pattern that requires an interior or boundary of each to meet is
    /// true only of meeting operands, so an index reads the pattern too.
    Relate,
}

impl SpatialOperation {
    /// Whether `f(column, value) = 1` holds only of a column value whose
    /// envelope meets the value's, so an index window finds every row;
    /// of `Equals` an empty value's window is every row.
    pub fn windowed(self) -> bool {
        match self {
            Self::Intersects | Self::Meets | Self::Equals => true,
            Self::Distance | Self::IsEmpty | Self::Relate => false,
        }
    }
}

/// A geometry with an opaque spatial reference identifier. Coordinates
/// remain in input order; assigning an SRID does not transform them.
/// The envelope is computed once, and absent for an empty point set.
#[derive(Clone, Debug)]
pub struct Geometry {
    srid: i32,
    shape: Shape,
    bounds: Option<Bounds>,
}

impl Geometry {
    /// Constructs a finite XY point without text or binary parsing.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if either coordinate is not finite.
    pub fn point(x: f64, y: f64, srid: i32) -> Result<Self> {
        Self::of(Shape::Point(Some(shape::coordinate([x, y])?)), srid)
    }

    /// Constructs a closed rectangular polygon from XY bounds. Both
    /// axes must have positive extent; no text or binary parsing occurs.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 for non-finite, reversed or zero-width bounds.
    pub fn rectangle(lower: [f64; DIMENSIONS], upper: [f64; DIMENSIONS], srid: i32) -> Result<Self> {
        let [min_x, min_y] = shape::coordinate(lower)?;
        let [max_x, max_y] = shape::coordinate(upper)?;
        if min_x >= max_x || min_y >= max_y {
            return Err(invalid("a rectangle needs a minimum strictly below its maximum on both axes"));
        }
        let ring = vec![[min_x, min_y], [min_x, max_y], [max_x, max_y], [max_x, min_y], [min_x, min_y]];
        Self::of(Shape::Polygon(vec![ring]), srid)
    }

    /// Changes the reference label without transforming coordinates.
    #[must_use]
    pub fn with_srid(mut self, srid: i32) -> Self {
        self.srid = srid;
        self
    }

    /// The XY coordinates of a point, absent for POINT EMPTY.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if this is not a point geometry.
    pub fn coordinates(&self) -> Result<Option<[f64; DIMENSIONS]>> {
        match &self.shape {
            Shape::Point(point) => Ok(*point),
            Shape::Line(_) | Shape::Polygon(_) | Shape::Multi(_, _) => Err(invalid("expected a POINT geometry")),
        }
    }

    /// The planar value of x and y of the well-known text `text`, of any
    /// type and coordinate dimension: the linear form of what
    /// ST_GeomFromText reads [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
    /// §5.1.62].
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 for malformed text, as the model's checks for a
    /// value that is no value of its type; SQLCODE -900 past a
    /// representation limit.
    pub fn from_wkt(text: &str, srid: i32) -> Result<Self> {
        Self::from_geom(&read_text(text, srid)?.linear()?.xy())
    }

    /// The planar value of x and y of the well-known binary `bytes`, of
    /// either byte order, any type and coordinate dimension: the linear
    /// form of what ST_GeomFromWKB reads [§5.1.63].
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF23 for malformed bytes, as the model's checks; SQLCODE
    /// -900 past a representation limit.
    pub fn from_wkb(bytes: &[u8], srid: i32) -> Result<Self> {
        limited(bytes.len(), MAX_BYTES)?;
        Self::from_geom(&model::read_wkb(bytes, srid)?.linear()?.xy())
    }

    fn from_canonical_wkb(bytes: &[u8], srid: i32) -> Result<Self> {
        limited(bytes.len(), MAX_BYTES)?;
        Self::of(wkb::read_canonical(bytes)?, srid)
    }

    fn of(shape: Shape, srid: i32) -> Result<Self> {
        let bounds = shape.bounds()?;
        Ok(Self { srid, shape, bounds })
    }

    /// The opaque spatial reference identifier; no CRS lookup is made.
    pub fn srid(&self) -> i32 {
        self.srid
    }

    /// The uppercase WKT name of the outer geometry's kind.
    pub fn geometry_type(&self) -> &'static str {
        self.shape.name()
    }

    /// Whether no component contains a coordinate; distinct from SQL NULL.
    pub fn is_empty(&self) -> bool {
        self.bounds.is_none()
    }

    /// The closed XY envelope, absent for an empty geometry.
    pub fn bounds(&self) -> Option<Bounds> {
        self.bounds
    }

    /// Whether the closed XY point sets share any point, including their
    /// boundaries, by the planar engine's exact ST_Intersects
    /// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.48]. Empty sets never
    /// intersect here; the SQL function returns the null value for them.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if the geometries have different SRIDs, even if empty;
    /// -910 if noding does not settle.
    pub fn intersects(&self, other: &Self) -> Result<bool> {
        Ok(self.intersects_located(other, None)?.unwrap_or(false))
    }

    /// ST_Intersects, none if either set is empty [§5.1.48 Description
    /// 2c]. Preparation may supply the same decision from its point
    /// locator; reference and empty-set checks still apply.
    fn intersects_located(&self, other: &Self, located: Option<bool>) -> Result<Option<bool>> {
        self.same_reference(other)?;
        let (Some(left), Some(right)) = (self.bounds, other.bounds) else { return Ok(None) };
        if !left.intersects(right) {
            return Ok(Some(false));
        }
        if let Some(known) = located {
            return Ok(Some(known));
        }
        crate::spatial::planar::intersects(&engine::of_shape(&self.shape), &engine::of_shape(&other.shape))
    }

    /// Minimum Cartesian distance in the supplied coordinate units,
    /// absent if either point set is empty. Intersecting sets have distance
    /// zero. Polygon topology must be valid. Measures are approximate
    /// binary64 values; assigning a geographic SRID does not change units.
    /// A finite result is at least the monotone lower bound between the
    /// geometries' envelopes ([`Bounds::distance_lower_bound`]).
    ///
    /// # Errors
    ///
    /// SQLCODE -301 for different SRIDs, even if empty, or SQLSTATE 22003
    /// if the resulting distance exceeds the finite binary64 range.
    pub fn distance(&self, other: &Self) -> Result<Option<f64>> {
        self.distance_located(other, None)
    }

    fn distance_located(&self, other: &Self, located: Option<bool>) -> Result<Option<f64>> {
        self.same_reference(other)?;
        let (Some(left), Some(right)) = (self.bounds, other.bounds) else { return Ok(None) };
        if self.intersects_located(other, located)? == Some(true) {
            return Ok(Some(0.0_f64));
        }
        let distance = distance::separated(&self.shape, &other.shape);
        if !distance.is_finite() {
            return Err(SqlError::new(Code::Data, "[EXTENSION] geometry distance exceeds the finite binary64 range")
                .with_state(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
        }
        // A monotone envelope bound also bounds the computed measure,
        // allowing distance pruning without a guessed rounding tolerance.
        Ok(Some(distance.max(left.distance_lower_bound(right))))
    }

    fn same_reference(&self, other: &Self) -> Result<()> {
        if self.srid != other.srid {
            return Err(invalid("spatial operands must have the same SRID"));
        }
        Ok(())
    }

    /// WKT with round-trippable coordinates. WKT does not carry the SRID.
    pub fn to_wkt(&self) -> String {
        wkt::write(&self.shape)
    }

    /// Little-endian WKB, including the NaN-pair empty-point convention.
    /// WKB does not carry the SRID.
    pub fn to_wkb(&self) -> Vec<u8> {
        wkb::write(&self.shape)
    }

    /// The SQL extension value; its versioned payload retains its SRID.
    pub fn to_value(&self) -> Value {
        kind::value(self)
    }

    /// Reads a GEOMETRY value, checking its format and structure.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if `value` is not GEOMETRY; -916 for a malformed or
    /// unknown-version GEOMETRY payload.
    pub fn from_value(value: &Value) -> Result<Self> {
        kind::geometry(value)
    }
}

pub(crate) fn invalid(detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] invalid geometry: {detail}"))
}

pub(crate) fn unsupported(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] geometry does not support {detail}"))
}

/// The value the well-known text `text` represents in the system
/// `srid`, as ST_GeomFromText reads it [ISO/IEC 13249-3:2016 (DIS, SC32
/// N2593) §5.1.62].
///
/// # Errors
///
/// SQLSTATE 2FF22 of malformed text; SQLCODE -900 past `MAX_BYTES`.
pub(crate) fn read_text(text: &str, srid: i32) -> Result<model::Geom> {
    limited(text.len(), MAX_BYTES)?;
    crate::spatial::formats::wkt::read_geom(text, srid)
}

fn limited(count: usize, maximum: usize) -> Result<()> {
    if count > maximum {
        return Err(unsupported("this representation size or nesting depth (RULES.md D85)"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
