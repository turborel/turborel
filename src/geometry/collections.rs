//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §10.1.3 to §10.1.5] The
//! observers and the mutator of a collection's elements: ST_NumGeometries,
//! ST_GeometryN and the two ST_Geometries methods, which ST_MultiPoint,
//! ST_MultiCurve, ST_MultiLineString, ST_MultiSurface and ST_MultiPolygon
//! override [§10.2.3, §10.3.8, §10.4.3, §10.5.11, §10.6.3]. Each is
//! called as a method (`geometry/methods.rs`) or as a function of the
//! collection first.

use super::model::{Body, Collection, Curve, Geom, Kind, Surface};
use super::{data_type, data_type_of, kind};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::ScalarFunction;
use crate::value::types::MAX_ARRAY_CARDINALITY;
use crate::value::{DataType, Decimal, Spelled, Value};

/// ST_MaxGeometryArrayElements: an array's most elements.
pub const MAX_GEOMETRY_ARRAY_ELEMENTS: u32 = MAX_ARRAY_CARDINALITY;
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// A routine of a collection's elements. `of` is the collection type
/// whose method it is: ST_GeomCollection's, or a subtype's overriding
/// ST_Geometries, whose array is of that subtype's element type.
#[derive(Clone, Copy, Debug)]
pub enum CollectionFunction {
    /// [§10.1.4] ST_NumGeometries(): the count of elements.
    NumGeometries,
    /// [§10.1.5] ST_GeometryN(INTEGER): the element at a position.
    GeometryN,
    /// [§10.1.3] ST_Geometries() and ST_Geometries(ST_Geometry ARRAY):
    /// the elements as an array, or the collection of an array's.
    Geometries(Kind),
}

/// [§10.1.3 to §10.6.3] The routines, ST_Geometries once for each type
/// that declares it.
pub const COLLECTION_FUNCTIONS: [CollectionFunction; 8] = [
    CollectionFunction::NumGeometries,
    CollectionFunction::GeometryN,
    CollectionFunction::Geometries(Kind::GeomCollection),
    CollectionFunction::Geometries(Kind::MultiPoint),
    CollectionFunction::Geometries(Kind::MultiCurve),
    CollectionFunction::Geometries(Kind::MultiLineString),
    CollectionFunction::Geometries(Kind::MultiSurface),
    CollectionFunction::Geometries(Kind::MultiPolygon),
];

/// The type of the elements of a collection of type `of`.
pub fn element_kind(of: Kind) -> Kind {
    match of {
        Kind::MultiPoint => Kind::Point,
        Kind::MultiCurve => Kind::Curve,
        Kind::MultiLineString => Kind::LineString,
        Kind::MultiSurface => Kind::Surface,
        Kind::MultiPolygon => Kind::Polygon,
        Kind::Geometry
        | Kind::Point
        | Kind::Curve
        | Kind::LineString
        | Kind::CircularString
        | Kind::Circle
        | Kind::GeodesicString
        | Kind::EllipticalCurve
        | Kind::NurbsCurve
        | Kind::Clothoid
        | Kind::SpiralCurve
        | Kind::CompoundCurve
        | Kind::Surface
        | Kind::CurvePolygon
        | Kind::Polygon
        | Kind::Triangle
        | Kind::PolyhdrlSurface
        | Kind::Tin
        | Kind::CompoundSurface
        | Kind::Solid
        | Kind::BRepSolid
        | Kind::GeomCollection => Kind::Geometry,
    }
}

/// An SQL/MM Spatial exception of `state`.
fn spatial(state: SqlState, msg: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §10.1] {msg}")).with_state(state)
}

fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §10.1] invalid arguments to {name}"))
}

/// The elements of `g`, none if it is no collection.
pub fn elements(g: &Geom) -> Option<Vec<Body>> {
    let Body::Collection(collection) = &g.body else { return None };
    Some(match collection {
        Collection::Geometry(v) => v.clone(),
        Collection::MultiPoint(v) => v.iter().map(|p| Body::Point(p.clone())).collect(),
        Collection::MultiCurve(v) => v.iter().map(|c| Body::Curve(c.clone())).collect(),
        Collection::MultiLineString(v) => v.iter().map(|c| Body::Curve(Curve::LineString(c.clone()))).collect(),
        Collection::MultiSurface(v) => v.iter().map(|s| Body::Surface(s.clone())).collect(),
        Collection::MultiPolygon(v) => v.iter().map(|r| Body::Surface(Surface::Polygon(r.clone()))).collect(),
    })
}

/// The collection of type `of` of `bodies`.
///
/// # Errors
///
/// SQLSTATE 2FF08 if an element is of no type the collection holds.
pub(super) fn collected(of: Kind, bodies: Vec<Body>) -> Result<Collection> {
    if bodies.iter().any(|b| !b.kind().is_subtype_of(element_kind(of))) {
        return Err(spatial(SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE, &format!("an element is of no type {} holds", of.sql_name())));
    }
    let curves = |all: Vec<Body>| all.into_iter().filter_map(|b| if let Body::Curve(c) = b { Some(c) } else { None });
    let surfaces = |all: Vec<Body>| all.into_iter().filter_map(|b| if let Body::Surface(s) = b { Some(s) } else { None });
    Ok(if of == Kind::MultiPoint {
        Collection::MultiPoint(bodies.into_iter().filter_map(|b| if let Body::Point(p) = b { Some(p) } else { None }).collect())
    } else if of == Kind::MultiCurve {
        Collection::MultiCurve(curves(bodies).collect())
    } else if of == Kind::MultiLineString {
        Collection::MultiLineString(curves(bodies).filter_map(linestring).collect())
    } else if of == Kind::MultiSurface {
        Collection::MultiSurface(surfaces(bodies).collect())
    } else if of == Kind::MultiPolygon {
        Collection::MultiPolygon(surfaces(bodies).filter_map(polygon).collect())
    } else {
        Collection::Geometry(bodies)
    })
}

fn linestring(c: Curve) -> Option<Vec<f64>> {
    if let Curve::LineString(p) = c { Some(p) } else { None }
}

/// The rings of a polygon, or of a triangle as a polygon of its ring.
fn polygon(s: Surface) -> Option<Vec<Vec<f64>>> {
    match s {
        Surface::Polygon(r) => Some(r),
        Surface::Triangle(r) => Some(vec![r]),
        Surface::CurvePolygon(_) | Surface::PolyhdrlSurface(_) | Surface::Tin(_) | Surface::Compound(_) => None,
    }
}

impl CollectionFunction {
    /// The array of elements of a collection of type `of`.
    fn array_of(of: Kind) -> DataType {
        DataType::array(data_type_of(element_kind(of)), MAX_GEOMETRY_ARRAY_ELEMENTS)
    }

    /// [§10.1.3 to §10.1.5] Of the collection `g` and the rest of the
    /// arguments, `rest`: the value, and the warning raised, if one.
    fn of(self, g: &Geom, rest: &[Value]) -> Result<(Value, Option<SqlState>)> {
        let Some(bodies) = elements(g) else {
            return Err(spatial(
                SqlState::SPATIAL_INVALID_ARGUMENT,
                &format!("{} of an {}, no collection", self.name(), g.kind().sql_name()),
            ));
        };
        // [§10.1.3 D2a, §10.1.4 D2a, §10.1.5 D2a] of the empty set, null.
        let empty = bodies.is_empty();
        match (self, rest) {
            (Self::NumGeometries, []) => Ok((
                if empty { Value::Null } else { Value::Exact(Decimal::new(i128::try_from(bodies.len()).unwrap_or(i128::MAX), 0)) },
                None,
            )),
            (Self::GeometryN, [position]) => Ok(nth(g, bodies, position)),
            (Self::Geometries(_), []) => {
                let values: Vec<Value> = bodies.into_iter().map(|b| g.part(b).to_value()).collect();
                Ok((if empty { Value::Null } else { Value::Array(values.into()) }, None))
            }
            (Self::Geometries(_), [array]) => Ok((mutated(g, array)?, None)),
            (Self::NumGeometries | Self::GeometryN | Self::Geometries(_), _) => Err(signature(self.name())),
        }
    }
}

/// [§10.1.5 D2] The element of `g` at `position`, of one; null and the
/// warning 01F01 at one out of its range.
fn nth(g: &Geom, bodies: Vec<Body>, position: &Value) -> (Value, Option<SqlState>) {
    if bodies.is_empty() {
        return (Value::Null, None);
    }
    let whole = if let Value::Exact(d) = position { (d.scale == 0).then_some(d.units) } else { None };
    let at = whole.and_then(|n| usize::try_from(n).ok()).and_then(|n| n.checked_sub(1));
    match at.and_then(|i| bodies.into_iter().nth(i)) {
        Some(b) => (g.part(b).to_value(), None),
        None => (Value::Null, Some(SqlState::SPATIAL_INVALID_POSITION)),
    }
}

/// [§18.1.1] ST_CheckNulls: that `array` is not null and holds no null.
///
/// # Errors
///
/// SQLSTATE 2FF03 if it is null or holds one.
fn check_nulls(array: &Value) -> Result<()> {
    let held = if let Value::Array(items) = array { items.iter().any(Value::is_null) } else { false };
    if array.is_null() || held {
        return Err(spatial(SqlState::SPATIAL_NULL_ARGUMENT, "the array is null or holds the null value"));
    }
    Ok(())
}

/// [§10.1.3 D4] `g` of the elements of `array`: of its type, SRID and,
/// of an empty array, coordinate dimension; the elements', else.
///
/// # Errors
///
/// SQLSTATE 2FF10 if an element's spatial reference system is not `g`'s; 2FF25 if two
/// elements differ in their coordinate dimension; 2FF08 if one is of no
/// type the collection holds.
fn mutated(g: &Geom, array: &Value) -> Result<Value> {
    let Value::Array(items) = array else { return Err(signature("ST_GEOMETRIES")) };
    let parts = items.iter().map(Geom::from_value).collect::<Result<Vec<_>>>()?;
    if parts.iter().any(|p| p.srid != g.srid) {
        return Err(spatial(SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, "mixed spatial reference systems"));
    }
    let dims = parts.first().map_or(g.dims, |p| p.dims);
    if parts.iter().any(|p| p.dims != dims) {
        return Err(spatial(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, "mixed coordinate dimensions"));
    }
    let body = Body::Collection(collected(g.kind(), parts.into_iter().map(|p| p.body).collect())?);
    Ok(Geom::new(g.srid, dims, body)?.to_value())
}

impl ScalarFunction for CollectionFunction {
    fn name(&self) -> &'static str {
        match self {
            Self::NumGeometries => "ST_NUMGEOMETRIES",
            Self::GeometryN => "ST_GEOMETRYN",
            Self::Geometries(_) => "ST_GEOMETRIES",
        }
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let Some(first) = args.first().filter(|t| data_type().kind().comparable(t)) else { return Err(signature(self.name())) };
        let of = super::of_data_type(first).filter(|k| k.is_subtype_of(Kind::GeomCollection) || *k == Kind::Geometry);
        let of = of.ok_or_else(|| signature(self.name()))?;
        match (self, args.len()) {
            (Self::NumGeometries, 1) => Ok(INTEGER),
            (Self::GeometryN, 2) => Ok(data_type()),
            (Self::Geometries(declared), 1) => Ok(Self::array_of(if of.is_subtype_of(*declared) { of } else { *declared })),
            (Self::Geometries(_), 2) => Ok(first.clone()),
            (Self::NumGeometries | Self::GeometryN | Self::Geometries(_), _) => Err(signature(self.name())),
        }
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        match (self, position) {
            (_, 0) => known.first().cloned().flatten().filter(|t| super::of_data_type(t).is_some()).or_else(|| Some(data_type())),
            (Self::GeometryN, 1) => Some(INTEGER),
            (Self::Geometries(_), 1) => Some(Self::array_of(Kind::GeomCollection)),
            (Self::NumGeometries | Self::GeometryN | Self::Geometries(_), _) => None,
        }
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_warned(args, None).map(|(v, _)| v)
    }

    fn warns(&self) -> bool {
        matches!(self, Self::GeometryN)
    }

    fn call_warned(&self, args: &[Value], _temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<(Value, Option<SqlState>)> {
        let [first, rest @ ..] = args else { return Err(signature(self.name())) };
        // [§10.1.3 D4a] the mutator's array is checked though SELF is null.
        if let (Self::Geometries(_), [array]) = (self, rest) {
            check_nulls(array)?;
        }
        if args.iter().any(Value::is_null) {
            return Ok((Value::Null, None));
        }
        kind::payload(first).ok_or_else(|| signature(self.name()))?;
        self.of(&Geom::from_value(first)?, rest)
    }

    fn deterministic(&self) -> bool {
        true
    }
}
