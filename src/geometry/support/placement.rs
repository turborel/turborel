//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.3] The routines of
//! ST_AffinePlacement: the constructor function, whose value is the empty
//! set, the constructor of a location and reference directions, their
//! observers and mutators, ST_InDimension, ST_OutDimension, ST_Transform
//! and ST_IsEmpty.
//!
//! ST_Transform(ST_Point) maps a point of the parameter space, its first
//! ST_InDimension coordinates, to the location plus each coordinate times
//! its reference direction: a point of the location's SRID and
//! dimensions [§17.3.7].

use super::Support::{AffinePlacement as P, Vector as V};
use super::arrays::{vector_coord_dim, vector_srid};
use super::ops::{Op, Systems, Ty, arg, elements, geometry, int, spatial};
use super::values::{Placement, Vector};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Geom, Kind};
use crate::value::Value;

const PLACEMENT: Ty = Ty::Of(P);
const POINT: Ty = Ty::Geom(Kind::Point);
const VECTORS: Ty = Ty::OfArray(V);

/// The routines.
pub(super) static OPS: &[Op] = &[
    Op::constructor(P, &[], |_, _| Ok(Placement::default().value())),
    Op::constructor(P, &[POINT, VECTORS], |a, _| {
        let mut p = Placement::default();
        set_location(&mut p, &geometry(a, 0)?)?;
        set_directions(&mut p, elements(a, 1)?)?;
        Ok(p.value())
    }),
    Op::method("ST_LOCATION", &[PLACEMENT], POINT, |a, _| {
        Ok(Placement::of(arg(a, 0)?)?.location.map_or(Value::Null, crate::geometry::kind::wrap))
    }),
    Op::method("ST_LOCATION", &[PLACEMENT, POINT], PLACEMENT, |a, _| {
        mutate(a, "§17.3.3 Description 4a", |p, a| set_location(p, &geometry(a, 1)?))
    })
    .called(),
    Op::method("ST_REFDIRECTIONS", &[PLACEMENT], VECTORS, |a, _| {
        let directions = Placement::of(arg(a, 0)?)?.directions;
        Ok(directions.map_or(Value::Null, |d| Value::Array(d.iter().map(Vector::value).collect::<Vec<_>>().into())))
    }),
    Op::method("ST_REFDIRECTIONS", &[PLACEMENT, VECTORS], PLACEMENT, |a, _| {
        mutate(a, "§17.3.4 Description 4a", |p, a| set_directions(p, elements(a, 1)?))
    })
    .called(),
    Op::method("ST_INDIMENSION", &[PLACEMENT], Ty::Integer, |a, _| {
        let directions = Placement::of(arg(a, 0)?)?.directions;
        Ok(directions.map_or(Value::Null, |d| int(i64::try_from(d.len()).unwrap_or(i64::MAX))))
    }),
    Op::method("ST_OUTDIMENSION", &[PLACEMENT], Ty::Integer, |a, _| {
        let Some(directions) = Placement::of(arg(a, 0)?)?.directions else { return Ok(Value::Null) };
        let values: Vec<Value> = directions.iter().map(Vector::value).collect();
        Ok(int(i64::from(vector_coord_dim(&values)?)))
    }),
    Op::method("ST_TRANSFORM", &[PLACEMENT, POINT], POINT, transform),
    Op::method("ST_ISEMPTY", &[PLACEMENT], Ty::Integer, |a, _| Ok(int(i64::from(Placement::of(arg(a, 0)?)? == Placement::default())))),
];

/// A mutator called on null input: a null argument is 2FF03, a null SELF
/// gives the null value.
fn mutate(args: &[Value], clause: &str, change: fn(&mut Placement, &[Value]) -> Result<()>) -> Result<Value> {
    if arg(args, 1)?.is_null() {
        return Err(spatial(SqlState::SPATIAL_NULL_ARGUMENT, clause, "null argument"));
    }
    if arg(args, 0)?.is_null() {
        return Ok(Value::Null);
    }
    let mut p = Placement::of(arg(args, 0)?)?;
    change(&mut p, args)?;
    Ok(p.value())
}

/// 2FF10 or 2FF25: the location and the directions differ in SRID or in
/// coordinate dimension [§17.3.3 Description 4d, §17.3.4 Description 4c, 4d].
fn check(location: &Geom, directions: &[Vector]) -> Result<()> {
    if directions.is_empty() {
        return Ok(());
    }
    let values: Vec<Value> = directions.iter().map(Vector::value).collect();
    if vector_srid(&values)? != location.srid {
        return Err(spatial(
            SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS,
            "§17.3.3 Description 4d i",
            "mixed spatial reference systems",
        ));
    }
    if vector_coord_dim(&values)? != location.dims.coordinate_dimension() {
        return Err(spatial(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, "§17.3.3 Description 4d ii", "mixed coordinate dimensions"));
    }
    Ok(())
}

/// [§17.3.3 Description 4] The location, of no m, checked against the
/// directions.
fn set_location(p: &mut Placement, location: &Geom) -> Result<()> {
    if location.dims.has_m() {
        return Err(spatial(SqlState::SPATIAL_M_COORDINATES_NOT_ALLOWED, "§17.3.3 Description 4c", "m coordinates not allowed"));
    }
    check(location, p.directions.as_deref().unwrap_or_default())?;
    p.location = Some(location.encode());
    Ok(())
}

/// [§17.3.4 Description 4] The directions, none null nor empty, checked
/// against the location.
fn set_directions(p: &mut Placement, given: &[Value]) -> Result<()> {
    vector_srid(given)?;
    let directions: Vec<Vector> = given.iter().map(Vector::of).collect::<Result<_>>()?;
    if let Some(bytes) = &p.location {
        check(&Geom::decode(bytes)?, &directions)?;
    }
    p.directions = Some(directions);
    Ok(())
}

/// [§17.3.7] ST_Transform(ST_Point).
fn transform(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let p = Placement::of(arg(args, 0)?)?;
    let (Some(location), Some(directions)) = (p.location, p.directions) else { return Ok(Value::Null) };
    let location = Geom::decode(&location)?;
    let Body::Point(origin) = &location.body else { return Ok(Value::Null) };
    let given = geometry(args, 1)?;
    let Body::Point(at) = &given.body else { return Ok(Value::Null) };
    let planar = given.dims.planar();
    if at.len() < directions.len() || directions.len() > planar {
        let msg = "invalid argument: the point has fewer coordinates than the placement's input dimension";
        return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§17.3.7", msg));
    }
    let mut out: Vec<f64> = origin.iter().copied().take(location.dims.planar()).collect();
    for (u, d) in at.iter().zip(&directions) {
        for (o, c) in out.iter_mut().zip([d.x, d.y, d.z].into_iter().flatten()) {
            *o += u * c;
        }
    }
    Ok(Geom::new(location.srid, location.dims, Body::Point(out))?.to_value())
}
