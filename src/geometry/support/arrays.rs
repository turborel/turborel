//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §18.1] The support routines of
//! arrays of geometry values, of ST_Vector values and of ST_NURBSPoint
//! values: ST_MaxDimension, ST_CheckSRID, ST_GetCoordDim, ST_GetIs3D,
//! ST_GetIsMeasured, the procedures ST_CheckNulls and ST_CheckConsecDups,
//! and the nineteen cast functions ST_ToPointAry to ST_ToBRepSolidAry.
//!
//! A null element of an array is passed over by ST_MaxDimension and
//! ST_GetCoordDim, as the clauses' comparisons of its dimension are
//! unknown. Each cast function returns its array's values as they are,
//! once each is found of its target type.

use std::sync::Arc;

use super::Support::{NurbsPoint as N, Vector as V};
use super::ops::{Op, Systems, Ty, arg, elements, int, spatial};
use super::values::{NurbsPoint, Vector};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::builtin::{Procedure, ProcedureContext};
use crate::geometry::model::{Body, Collection, Geom, Kind};
use crate::value::{DataType, Value};

const GEOMETRIES: Ty = Ty::GeomArray(Kind::Geometry);
const VECTORS: Ty = Ty::OfArray(V);

/// The routines.
pub(super) static OPS: &[Op] = &[
    Op::function("ST_MAXDIMENSION", &[GEOMETRIES], Ty::SmallInt, max_dimension),
    Op::function("ST_CHECKSRID", &[GEOMETRIES], Ty::Integer, |a, _| Ok(int(i64::from(geometry_srid(arg(a, 0)?)?)))).called(),
    Op::function("ST_CHECKSRID", &[VECTORS], Ty::Integer, |a, _| {
        Ok(int(i64::from(vector_srid(present(arg(a, 0)?, "§18.1.6 Description 4a")?)?)))
    })
    .called(),
    Op::function("ST_GETCOORDDIM", &[GEOMETRIES], Ty::SmallInt, |a, _| Ok(int(i64::from(geometry_coord_dim(&geometries(a, 0)?)?)))),
    Op::function("ST_GETCOORDDIM", &[Ty::Geom(Kind::Geometry), GEOMETRIES], Ty::SmallInt, |a, _| {
        let mut all = vec![Geom::from_value(arg(a, 0)?)?];
        all.extend(geometries(a, 1)?);
        Ok(int(i64::from(geometry_coord_dim(&all)?)))
    }),
    Op::function("ST_GETCOORDDIM", &[VECTORS], Ty::SmallInt, |a, _| Ok(int(i64::from(vector_coord_dim(elements(a, 0)?)?)))),
    Op::function("ST_GETCOORDDIM", &[Ty::OfArray(N)], Ty::SmallInt, nurbs_coord_dim),
    Op::function("ST_GETIS3D", &[GEOMETRIES], Ty::SmallInt, |a, _| first_of(a, |g| g.dims.has_z())),
    Op::function("ST_GETISMEASURED", &[GEOMETRIES], Ty::SmallInt, |a, _| first_of(a, |g| g.dims.has_m())),
    Op::function("ST_TOPOINTARY", &[GEOMETRIES], Ty::GeomArray(Kind::Point), |a, _| to_array(a, Kind::Point)),
    Op::function("ST_TOCURVEARY", &[GEOMETRIES], Ty::GeomArray(Kind::Curve), |a, _| to_array(a, Kind::Curve)),
    Op::function("ST_TOLINESTRINGARY", &[GEOMETRIES], Ty::GeomArray(Kind::LineString), |a, _| to_array(a, Kind::LineString)),
    Op::function("ST_TOCIRCULARARY", &[GEOMETRIES], Ty::GeomArray(Kind::CircularString), |a, _| to_array(a, Kind::CircularString)),
    Op::function("ST_TOCIRCLEARY", &[GEOMETRIES], Ty::GeomArray(Kind::Circle), |a, _| to_array(a, Kind::Circle)),
    Op::function("ST_TOGEODESICARY", &[GEOMETRIES], Ty::GeomArray(Kind::GeodesicString), |a, _| to_array(a, Kind::GeodesicString)),
    Op::function("ST_TOELLIPTICALARY", &[GEOMETRIES], Ty::GeomArray(Kind::EllipticalCurve), |a, _| to_array(a, Kind::EllipticalCurve)),
    Op::function("ST_TONURBSARY", &[GEOMETRIES], Ty::GeomArray(Kind::NurbsCurve), |a, _| to_array(a, Kind::NurbsCurve)),
    Op::function("ST_TOCLOTHOIDARY", &[GEOMETRIES], Ty::GeomArray(Kind::Clothoid), |a, _| to_array(a, Kind::Clothoid)),
    Op::function("ST_TOSPIRALARY", &[GEOMETRIES], Ty::GeomArray(Kind::SpiralCurve), |a, _| to_array(a, Kind::SpiralCurve)),
    Op::function("ST_TOCOMPOUNDARY", &[GEOMETRIES], Ty::GeomArray(Kind::CompoundCurve), |a, _| to_array(a, Kind::CompoundCurve)),
    Op::function("ST_TOSURFACEARY", &[GEOMETRIES], Ty::GeomArray(Kind::Surface), |a, _| to_array(a, Kind::Surface)),
    Op::function("ST_TOCURVEPOLYARY", &[GEOMETRIES], Ty::GeomArray(Kind::CurvePolygon), |a, _| to_array(a, Kind::CurvePolygon)),
    Op::function("ST_TOPOLYGONARY", &[GEOMETRIES], Ty::GeomArray(Kind::Polygon), |a, _| to_array(a, Kind::Polygon)),
    Op::function("ST_TOTRIANGLEARY", &[GEOMETRIES], Ty::GeomArray(Kind::Triangle), |a, _| to_array(a, Kind::Triangle)),
    Op::function("ST_TOPOLYHDRLARY", &[GEOMETRIES], Ty::GeomArray(Kind::PolyhdrlSurface), |a, _| to_array(a, Kind::PolyhdrlSurface)),
    Op::function("ST_TOTINARY", &[GEOMETRIES], Ty::GeomArray(Kind::Tin), |a, _| to_array(a, Kind::Tin)),
    Op::function("ST_TOCOMPSURFARY", &[GEOMETRIES], Ty::GeomArray(Kind::CompoundSurface), |a, _| to_array(a, Kind::CompoundSurface)),
    Op::function("ST_TOBREPSOLIDARY", &[GEOMETRIES], Ty::GeomArray(Kind::BRepSolid), |a, _| to_array(a, Kind::BRepSolid)),
];

/// [§4.2] The dimension of a geometry: -1 of the empty set, else its
/// type's, else of a collection the greatest of its elements'.
fn dimension(body: &Body) -> i16 {
    if body.is_empty() {
        return -1;
    }
    if let Some(d) = body.kind().dimension() {
        return d;
    }
    let Body::Collection(c) = body else { return -1 };
    match c {
        Collection::Geometry(parts) => parts.iter().map(dimension).max().unwrap_or(-1),
        Collection::MultiPoint(_) => 0,
        Collection::MultiCurve(_) | Collection::MultiLineString(_) => 1,
        Collection::MultiSurface(_) | Collection::MultiPolygon(_) => 2,
    }
}

/// The geometries of argument `at`, its null elements passed over.
fn geometries(args: &[Value], at: usize) -> Result<Vec<Geom>> {
    elements(args, at)?.iter().filter(|v| !v.is_null()).map(Geom::from_value).collect()
}

/// [§18.1.1] ST_MaxDimension: -1 of an empty array.
fn max_dimension(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let max = geometries(args, 0)?.iter().map(|g| dimension(&g.body)).max().unwrap_or(-1);
    Ok(int(i64::from(max)))
}

/// 2FF25 mixed coordinate dimensions, citing `clause`.
fn mixed(clause: &str) -> SqlError {
    spatial(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, clause, "mixed coordinate dimensions")
}

/// 2FF10 mixed spatial reference systems.
fn mixed_systems() -> SqlError {
    spatial(SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, "§18.1.2 Description 2b ii", "mixed spatial reference systems")
}

/// [§18.1.3 Description 2] ST_GetCoordDim of geometries: 2 of none, else
/// the first's, all of one ST_Is3D and ST_IsMeasured.
fn geometry_coord_dim(all: &[Geom]) -> Result<i16> {
    let Some(first) = all.first() else { return Ok(2) };
    let shape = |g: &Geom| (g.dims.has_z(), g.dims.has_m());
    if all.iter().any(|g| shape(g) != shape(first)) {
        return Err(mixed("§18.1.3 Description 2b i 2"));
    }
    Ok(first.dims.coordinate_dimension())
}

/// [§18.1.4, §18.1.5] ST_GetIs3D and ST_GetIsMeasured: 0 if the array's
/// coordinate dimension is 2, else of its first element.
fn first_of(args: &[Value], of: fn(&Geom) -> bool) -> Result<Value> {
    let all = geometries(args, 0)?;
    let dim = geometry_coord_dim(&all)?;
    let first = all.first().filter(|_| dim != 2).is_some_and(of);
    Ok(int(i64::from(first)))
}

/// [§18.1.6] ST_CheckNulls: `array` is not null; no element is null or
/// the empty set.
fn check_nulls(array: &Value, empty: fn(&Value) -> Result<bool>) -> Result<&[Value]> {
    let items = present(array, "§18.1.6 Description 2a")?;
    if items.iter().any(Value::is_null) {
        return Err(spatial(SqlState::SPATIAL_ELEMENT_IS_A_NULL_VALUE, "§18.1.6 Description 2b", "element is a null value"));
    }
    for v in items {
        if empty(v)? {
            return Err(spatial(SqlState::SPATIAL_ELEMENT_IS_AN_EMPTY_SET, "§18.1.6 Description 2c", "element is an empty set"));
        }
    }
    Ok(items)
}

/// The elements of `array`, 2FF03 null argument if it is null.
fn present<'a>(array: &'a Value, clause: &str) -> Result<&'a [Value]> {
    if let Value::Array(items) = array {
        return Ok(items);
    }
    if array.is_null() {
        return Err(spatial(SqlState::SPATIAL_NULL_ARGUMENT, clause, "null argument"));
    }
    Err(SqlError::new(Code::Data, format!("[EXTENSION] {array} is not an array")))
}

/// Whether a geometry value is the empty set.
fn geometry_empty(v: &Value) -> Result<bool> {
    Geom::from_value(v).map(|g| g.is_empty())
}

/// Whether an ST_Vector value is the empty set.
fn vector_empty(v: &Value) -> Result<bool> {
    Vector::of(v).map(|v| v.is_empty())
}

/// [§18.1.2 Description 2] ST_CheckSRID of geometries.
fn geometry_srid(array: &Value) -> Result<i32> {
    let all: Vec<Geom> = check_nulls(array, geometry_empty)?.iter().map(Geom::from_value).collect::<Result<_>>()?;
    let srid = all.first().map_or(0_i32, |g| g.srid);
    if all.iter().any(|g| g.srid != srid) {
        return Err(mixed_systems());
    }
    Ok(srid)
}

/// [§18.1.2 Description 4] ST_CheckSRID of vectors.
pub(super) fn vector_srid(items: &[Value]) -> Result<i32> {
    let all: Vec<Vector> = check_nulls(&Value::Array(items.into()), vector_empty)?.iter().map(Vector::of).collect::<Result<_>>()?;
    let srid = all.first().map_or(0_i32, |v| v.srid);
    if all.iter().any(|v| v.srid != srid) {
        return Err(mixed_systems());
    }
    Ok(srid)
}

/// [§18.1.3 Description 6] ST_GetCoordDim of vectors: 2 of none, else 2
/// or 3, all of one ST_Is3D.
pub(super) fn vector_coord_dim(items: &[Value]) -> Result<i16> {
    let z: Vec<bool> = items.iter().filter(|v| !v.is_null()).map(|v| Vector::of(v).map(|v| v.z.is_some())).collect::<Result<_>>()?;
    coord_dim_of(&z, "§18.1.3 Description 6b i 2")
}

/// The coordinate dimension of elements whose ST_Is3D are `z`.
fn coord_dim_of(z: &[bool], clause: &str) -> Result<i16> {
    let Some(first) = z.first() else { return Ok(2) };
    if z.iter().any(|z| z != first) {
        return Err(mixed(clause));
    }
    Ok(if *first { 3 } else { 2 })
}

/// [§18.1.3 Description 8] ST_GetCoordDim of NURBS points, by their
/// weighted points.
fn nurbs_coord_dim(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let mut z = Vec::new();
    for v in elements(args, 0)?.iter().filter(|v| !v.is_null()) {
        if let Some(point) = NurbsPoint::of(v)?.point {
            z.push(Geom::decode(&point)?.dims.has_z());
        }
    }
    Ok(int(i64::from(coord_dim_of(&z, "§18.1.3 Description 8b i 2")?)))
}

/// The SQLSTATE of an element not of `kind` [§18.1.8 to §18.1.26].
fn not_of_state(kind: Kind) -> SqlState {
    match kind {
        Kind::Triangle => SqlState::SPATIAL_NOT_A_TRIANGLE,
        Kind::PolyhdrlSurface | Kind::CompoundSurface | Kind::BRepSolid => SqlState::SPATIAL_NOT_A_POLYHDRLSURFACE,
        Kind::Tin => SqlState::SPATIAL_NOT_A_TIN,
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
        | Kind::Solid
        | Kind::GeomCollection
        | Kind::MultiPoint
        | Kind::MultiCurve
        | Kind::MultiLineString
        | Kind::MultiSurface
        | Kind::MultiPolygon => SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE,
    }
}

/// [§18.1.8 to §18.1.26] ST_ToPointAry and its peers: the array, its
/// elements checked by ST_CheckNulls and each of type `kind`.
fn to_array(args: &[Value], kind: Kind) -> Result<Value> {
    let items = check_nulls(arg(args, 0)?, geometry_empty)?;
    for v in items {
        let of = crate::geometry::kind::payload(v).and_then(crate::geometry::most_specific);
        if !of.is_some_and(|k| k.is_subtype_of(kind)) {
            let msg = format!("element is not an {} type", kind.sql_name());
            return Err(spatial(not_of_state(kind), "§18.1.8 Description 2b i", &msg));
        }
    }
    Ok(Value::Array(Arc::from(items)))
}

/// [§18.1.6, §18.1.7] The procedures ST_CheckNulls and ST_CheckConsecDups.
#[derive(Debug, Clone, Copy)]
pub enum ArrayProcedure {
    /// ST_CheckNulls(ST_Geometry ARRAY) and ST_CheckNulls(ST_Vector ARRAY).
    CheckNulls,
    /// ST_CheckConsecDups(ST_Geometry ARRAY).
    CheckConsecDups,
}

/// The procedures, built in.
pub fn procedures() -> Vec<Arc<dyn Procedure>> {
    vec![Arc::new(ArrayProcedure::CheckNulls), Arc::new(ArrayProcedure::CheckConsecDups)]
}

impl Procedure for ArrayProcedure {
    fn name(&self) -> &str {
        match self {
            Self::CheckNulls => "ST_CHECKNULLS",
            Self::CheckConsecDups => "ST_CHECKCONSECDUPS",
        }
    }

    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        (count == 1).then(|| vec![GEOMETRIES.data_type()])
    }

    fn overload(&self, args: &[DataType]) -> Option<Vec<DataType>> {
        let [DataType::Array(a)] = args else { return None };
        (matches!(self, Self::CheckNulls) && super::of_data_type(&a.element) == Some(V)).then(|| vec![VECTORS.data_type()])
    }

    fn call(&self, args: &[Value], _: &mut dyn ProcedureContext) -> Result<()> {
        let array = args.first().unwrap_or(&Value::Null);
        let vectors = if let Value::Array(items) = array { items.first().is_some_and(|v| super::of_value(v).is_some()) } else { false };
        if vectors {
            return check_nulls(array, vector_empty).map(drop);
        }
        let items = check_nulls(array, geometry_empty)?;
        if matches!(self, Self::CheckNulls) {
            return Ok(());
        }
        for pair in items.windows(2) {
            let [a, b] = pair else { continue };
            if crate::geometry::EQUALITY(&Geom::from_value(a)?, &Geom::from_value(b)?)? {
                return Err(spatial(SqlState::SPATIAL_DUPLICATE_VALUE, "§18.1.7 Description 2b", "duplicate value"));
            }
        }
        Ok(())
    }
}
