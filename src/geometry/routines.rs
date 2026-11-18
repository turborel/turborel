//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1, §7.1, §8.1, §8.3, §10.3,
//! §10.5, §10.6] The routines the planar engine (`spatial::planar`)
//! computes: the relationships, the set operations, ST_Buffer,
//! ST_ConvexHull, ST_Boundary, ST_IsValid, ST_IsSimple, ST_IsRing, the
//! measures and the polygons built from rings. Each is a function of its
//! subject first and a method of the type that declares it
//! (`methods.rs`). Each takes the linear form of x and y of its values
//! (`engine::engine_of`) and returns the null value where its clause
//! says so, for the empty set mostly.

mod eval;
mod results;
mod transform;
mod units;

pub use transform::Transform;
pub(super) use units::{fault, length_in};

use super::model::Kind;
use super::{GeometryFunction, data_type, data_type_of, fixed_srid, of_data_type};
use crate::error::{Code, Result, SqlError};
use crate::functions::ScalarFunction;
use crate::params::ParameterType;
use crate::value::{DOUBLE_PRECISION, DataType, Spelled, Value};

const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
/// ST_MaxUnitNameLength, implementation-defined [§5.1.30 DR 1].
const UNIT_NAME_LENGTH: u32 = 128;

/// The planar engine's routines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Routine {
    /// ST_Relate(g, g, matrix) [§5.1.45].
    Relate,
    /// ST_Equals [§5.1.43].
    Equals,
    /// ST_Touches [§5.1.50].
    Touches,
    /// ST_Crosses [§5.1.51].
    Crosses,
    /// ST_Within [§5.1.52].
    Within,
    /// ST_Contains [§5.1.53].
    Contains,
    /// ST_Overlaps [§5.1.54].
    Overlaps,
    /// ST_Intersection [§5.1.31].
    Intersection,
    /// ST_Union [§5.1.33].
    Union,
    /// ST_Difference [§5.1.35].
    Difference,
    /// ST_SymDifference [§5.1.37].
    SymDifference,
    /// ST_Buffer(g, distance [, unit]) [§5.1.30].
    Buffer,
    /// ST_ConvexHull [§5.1.29].
    ConvexHull,
    /// ST_Boundary [§5.1.17].
    Boundary,
    /// ST_IsValid [§5.1.10].
    IsValid,
    /// ST_IsSimple [§5.1.8].
    IsSimple,
    /// ST_IsRing of a curve [§7.1.8].
    IsRing,
    /// ST_Area([unit]) of a surface or multisurface [§8.1.2, §10.5.3].
    Area,
    /// ST_Length([unit]) of a curve or multicurve [§7.1.2, §10.3.5].
    Length,
    /// ST_Perimeter([unit]) of a surface or multisurface [§8.1.4, §10.5.5].
    Perimeter,
    /// ST_Centroid of a surface or multisurface [§8.1.6, §10.5.7].
    Centroid,
    /// ST_PointOnSurface of a surface or multisurface [§8.1.8, §10.5.9].
    PointOnSurface,
    /// ST_BdPolyFromText(text [, srid]) [§8.3.9].
    BdPolyFromText,
    /// ST_BdPolyFromWKB(binary [, srid]) [§8.3.10].
    BdPolyFromWkb,
    /// ST_BdMPolyFromText(text [, srid]) [§10.6.7].
    BdMPolyFromText,
    /// ST_BdMPolyFromWKB(binary [, srid]) [§10.6.8].
    BdMPolyFromWkb,
}

/// Every routine, each a built-in function.
pub const ROUTINES: [Routine; 26] = [
    Routine::Relate,
    Routine::Equals,
    Routine::Touches,
    Routine::Crosses,
    Routine::Within,
    Routine::Contains,
    Routine::Overlaps,
    Routine::Intersection,
    Routine::Union,
    Routine::Difference,
    Routine::SymDifference,
    Routine::Buffer,
    Routine::ConvexHull,
    Routine::Boundary,
    Routine::IsValid,
    Routine::IsSimple,
    Routine::IsRing,
    Routine::Area,
    Routine::Length,
    Routine::Perimeter,
    Routine::Centroid,
    Routine::PointOnSurface,
    Routine::BdPolyFromText,
    Routine::BdPolyFromWkb,
    Routine::BdMPolyFromText,
    Routine::BdMPolyFromWkb,
];

/// What a routine's parameter takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Param {
    Geometry,
    Double,
    Text,
}

/// What a routine is: its subject, its parameters and its result.
enum Shape {
    /// Two geometries, a third parameter if any, and the result.
    Pair(Option<Param>, DataType),
    /// One geometry of these types, then parameters, optional ones last.
    One(&'static [Kind], &'static [Param], &'static [Param], DataType),
    /// The ST_GeomFromText or ST_GeomFromWKB constructor it builds on.
    Rings(GeometryFunction, Kind),
}

const ANY: &[Kind] = &[Kind::Geometry];
const CURVES: &[Kind] = &[Kind::Curve, Kind::MultiCurve];
const SURFACES: &[Kind] = &[Kind::Surface, Kind::MultiSurface];
const UNIT: &[Param] = &[Param::Text];

impl Routine {
    fn shape(self) -> Shape {
        let geometry = data_type();
        match self {
            Self::Relate => Shape::Pair(Some(Param::Text), INTEGER),
            Self::Equals | Self::Touches | Self::Crosses | Self::Within | Self::Contains | Self::Overlaps => Shape::Pair(None, INTEGER),
            Self::Intersection | Self::Union | Self::Difference | Self::SymDifference => Shape::Pair(None, geometry),
            Self::Buffer => Shape::One(ANY, &[Param::Double], UNIT, geometry),
            Self::ConvexHull | Self::Boundary => Shape::One(ANY, &[], &[], geometry),
            Self::IsValid | Self::IsSimple => Shape::One(ANY, &[], &[], INTEGER),
            Self::IsRing => Shape::One(&[Kind::Curve], &[], &[], INTEGER),
            Self::Length => Shape::One(CURVES, &[], UNIT, DOUBLE),
            Self::Area | Self::Perimeter => Shape::One(SURFACES, &[], UNIT, DOUBLE),
            Self::Centroid | Self::PointOnSurface => Shape::One(SURFACES, &[], &[], data_type_of(Kind::Point)),
            Self::BdPolyFromText => Shape::Rings(GeometryFunction::FromText, Kind::Polygon),
            Self::BdPolyFromWkb => Shape::Rings(GeometryFunction::FromBinary, Kind::Polygon),
            Self::BdMPolyFromText => Shape::Rings(GeometryFunction::FromText, Kind::MultiPolygon),
            Self::BdMPolyFromWkb => Shape::Rings(GeometryFunction::FromBinary, Kind::MultiPolygon),
        }
    }

    /// The types a geometry subject may have, for a routine taking one.
    fn subjects(self) -> &'static [Kind] {
        match self.shape() {
            Shape::One(kinds, ..) => kinds,
            Shape::Pair(..) | Shape::Rings(..) => ANY,
        }
    }

    /// The parameters of a call of `count` arguments, if it has that many.
    fn params(self, count: usize) -> Option<Vec<Param>> {
        let (required, optional): (Vec<Param>, &[Param]) = match self.shape() {
            Shape::Pair(third, _) => ([Param::Geometry, Param::Geometry].into_iter().chain(third).collect(), &[]),
            Shape::One(_, required, optional, _) => (std::iter::once(Param::Geometry).chain(required.iter().copied()).collect(), optional),
            Shape::Rings(..) => return None,
        };
        let extra = count.checked_sub(required.len())?;
        (extra <= optional.len()).then(|| required.into_iter().chain(optional.iter().copied().take(extra)).collect())
    }

    fn declared(param: Param) -> DataType {
        match param {
            Param::Geometry => data_type(),
            Param::Double => DOUBLE,
            Param::Text => DataType::varying(UNIT_NAME_LENGTH),
        }
    }

    fn accepts(param: Param, t: &DataType) -> bool {
        match param {
            Param::Geometry => data_type().kind().comparable(t),
            Param::Double => t.is_numeric(),
            Param::Text => t.is_character(),
        }
    }

    /// Whether a subject of declared type `t` may hold a value this
    /// routine takes: a type above or below one it names.
    fn may_take(self, t: &DataType) -> bool {
        of_data_type(t).is_none_or(|kind| self.subjects().iter().any(|wanted| kind.is_subtype_of(*wanted) || wanted.is_subtype_of(kind)))
    }
}

fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1] invalid arguments to {name}"))
}

impl ScalarFunction for Routine {
    fn spatial_operation(&self) -> Option<super::SpatialOperation> {
        eval::spatial_operation(*self)
    }

    fn name(&self) -> &'static str {
        eval::name(*self)
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let shape = self.shape();
        if let Shape::Rings(constructor, kind) = shape {
            constructor.return_type(args)?;
            return Ok(data_type_of(kind));
        }
        let params = self.params(args.len()).ok_or_else(|| signature(self.name()))?;
        let fits = params.iter().zip(args).all(|(param, t)| Self::accepts(*param, t));
        if !fits || !args.first().is_some_and(|t| self.may_take(t)) {
            return Err(signature(self.name()));
        }
        match shape {
            Shape::Pair(_, result) | Shape::One(_, _, _, result) => Ok(result),
            Shape::Rings(..) => Err(signature(self.name())),
        }
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        if let Shape::Rings(constructor, _) = self.shape() {
            return constructor.argument_type(position, known);
        }
        let param = *self.params(known.len().max(position.saturating_add(1)))?.get(position)?;
        // Keep a column's constrained descriptor, which index planning
        // reads as its proof of the column's SRID (D85).
        let constrained = known
            .get(position)
            .and_then(Option::as_ref)
            .filter(|t| param == Param::Geometry && data_type().kind().comparable(t) && fixed_srid(t).is_some());
        Some(constrained.cloned().unwrap_or_else(|| Self::declared(param)))
    }

    fn parameter_type(&self, position: usize, known: &[Option<DataType>]) -> Option<ParameterType> {
        if let Shape::Rings(constructor, _) = self.shape() {
            return constructor.parameter_type(position, known);
        }
        self.argument_type(position, known).map(ParameterType::Of)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        eval::call(*self, args, None)
    }

    /// A unit form reads the statement's reference systems.
    fn uses_context(&self, arity: usize) -> bool {
        match self.shape() {
            Shape::One(_, required, optional, _) => {
                !optional.is_empty() && arity == required.len().saturating_add(1).saturating_add(optional.len())
            }
            Shape::Pair(..) | Shape::Rings(..) => false,
        }
    }

    fn call_in(&self, args: &[Value], cx: &dyn crate::functions::CallContext) -> Result<Value> {
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        eval::call(*self, args, cx.spatial())
    }
}

/// The routines, as built-in functions.
pub(crate) fn functions() -> impl Iterator<Item = std::sync::Arc<dyn ScalarFunction>> {
    let transform: std::sync::Arc<dyn ScalarFunction> = std::sync::Arc::new(Transform);
    ROUTINES
        .into_iter()
        .map(|routine| -> std::sync::Arc<dyn ScalarFunction> { std::sync::Arc::new(routine) })
        .chain(std::iter::once(transform))
}
