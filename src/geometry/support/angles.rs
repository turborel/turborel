//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §16.1] The routines of
//! ST_Angle, each the engine's (`spatial::srs::angle`): the nine
//! constructors, the observers and mutators of radians, degrees and
//! gradians, the components, ST_String, the arithmetic, the text and GML
//! representations, ST_AngleFromText, ST_AngleFromGML and
//! ST_OrderingCompare [§16.1.19], the function the type is ordered by.

use std::ops::Add;

use super::Support::{Angle as A, Direction as D};
use super::ops::{Op, Systems, Ty, arg, clob, double, fault, geometry, int, integer, text, varchar};
use super::values::{angle_of, angle_value, direction_of};
use crate::error::Result;
use crate::geometry::model::{Body, Curve, Geom, Kind};
use crate::spatial::srs::angle::Angle;
use crate::spatial::srs::{Condition, Fault};
use crate::value::Value;

/// ST_MaxAngleAsText, implementation-defined.
pub const MAX_ANGLE_AS_TEXT: u32 = 64;
/// ST_MaxAngleAsGML, implementation-defined.
pub const MAX_ANGLE_AS_GML: u32 = 4096;
/// ST_MaxAngleString, the engine's (`angle::MAX_ANGLE_STRING`).
pub const MAX_ANGLE_STRING: u32 = 64;

const ANGLE: Ty = Ty::Of(A);
const TEXT: Ty = Ty::Varchar(MAX_ANGLE_AS_TEXT);
const GML: Ty = Ty::Clob(MAX_ANGLE_AS_GML);
const POINT: Ty = Ty::Geom(Kind::Point);
const LINE: Ty = Ty::Geom(Kind::LineString);

/// The routines.
pub(super) static OPS: &[Op] = &[
    Op::constructor(A, &[Ty::Double], |a, _| Ok(angle_value(Angle::new(double(a, 0)?)))),
    Op::constructor(A, &[Ty::Char1, Ty::Double], with_units),
    Op::constructor(A, &[Ty::Integer, Ty::Double], degrees_minutes),
    Op::constructor(A, &[Ty::Integer, Ty::Integer, Ty::Double], dms),
    Op::constructor(A, &[POINT, POINT, POINT], at_point),
    Op::constructor(A, &[Ty::Of(D), Ty::Of(D)], |a, _| {
        Ok(angle_value(Angle::between(direction_of(arg(a, 0)?)?, direction_of(arg(a, 1)?)?)))
    }),
    Op::constructor(A, &[LINE, LINE], between_lines),
    Op::constructor(A, &[TEXT], from_text),
    Op::constructor(A, &[GML], from_gml),
    Op::method("ST_RADIANS", &[ANGLE], Ty::Double, |a, _| Ok(Value::Approx(angle_of(arg(a, 0)?)?.radians()))),
    Op::method("ST_RADIANS", &[ANGLE, Ty::Double], ANGLE, |a, _| Ok(angle_value(Angle::new(double(a, 1)?)))),
    Op::method("ST_DEGREES", &[ANGLE], Ty::Double, |a, _| Ok(Value::Approx(angle_of(arg(a, 0)?)?.degrees()))),
    Op::method("ST_DEGREES", &[ANGLE, Ty::Double], ANGLE, |a, _| Ok(angle_value(Angle::from_degrees(double(a, 1)?)))),
    Op::method("ST_GRADIANS", &[ANGLE], Ty::Double, |a, _| Ok(Value::Approx(angle_of(arg(a, 0)?)?.gradians()))),
    Op::method("ST_GRADIANS", &[ANGLE, Ty::Double], ANGLE, |a, _| Ok(angle_value(Angle::from_gradians(double(a, 1)?)))),
    Op::method("ST_DEGREECOMPONENT", &[ANGLE], Ty::Integer, |a, _| {
        Ok(int(i64::from(angle_of(arg(a, 0)?)?.degree_component().map_err(|f| fault(&f))?)))
    }),
    Op::method("ST_MINUTECOMPONENT", &[ANGLE], Ty::Integer, |a, _| Ok(int(i64::from(angle_of(arg(a, 0)?)?.minute_component())))),
    Op::method("ST_SECONDCOMPONENT", &[ANGLE], Ty::Double, |a, _| Ok(Value::Approx(angle_of(arg(a, 0)?)?.second_component()))),
    Op::method("ST_STRING", &[ANGLE], Ty::Varchar(MAX_ANGLE_STRING), |a, _| string(arg(a, 0)?, 2)),
    Op::method("ST_STRING", &[ANGLE, Ty::Integer], Ty::Varchar(MAX_ANGLE_STRING), |a, _| string(arg(a, 0)?, integer(a, 1)?)),
    Op::method("ST_STRING", &[ANGLE, Ty::Varchar(MAX_ANGLE_STRING)], ANGLE, |a, _| {
        Ok(angle_value(Angle::from_string(&text(a, 1)?).map_err(|f| fault(&f))?))
    }),
    Op::method("ST_ADD", &[ANGLE, ANGLE], ANGLE, |a, _| Ok(angle_value(angle_of(arg(a, 0)?)?.add(angle_of(arg(a, 1)?)?)))),
    Op::method("ST_SUBTRACT", &[ANGLE, ANGLE], ANGLE, |a, _| Ok(angle_value(angle_of(arg(a, 0)?)?.subtract(angle_of(arg(a, 1)?)?)))),
    Op::method("ST_MULTIPLY", &[ANGLE, Ty::Double], ANGLE, |a, _| Ok(angle_value(angle_of(arg(a, 0)?)?.multiply(double(a, 1)?)))),
    Op::method("ST_DIVIDE", &[ANGLE, Ty::Double], ANGLE, |a, _| {
        Ok(angle_value(angle_of(arg(a, 0)?)?.divide(double(a, 1)?).map_err(|f| fault(&f))?))
    }),
    Op::method("ST_ASTEXT", &[ANGLE], TEXT, |a, _| Ok(varchar(angle_of(arg(a, 0)?)?.as_text()))),
    Op::method("ST_GMLTOSQL", &[ANGLE, GML], ANGLE, |a, s| from_gml(a.get(1..).unwrap_or_default(), s)),
    Op::method("ST_ASGML", &[ANGLE], GML, |a, _| Ok(clob(&angle_of(arg(a, 0)?)?.as_gml()))),
    Op::function("ST_ANGLEFROMTEXT", &[TEXT], ANGLE, from_text),
    Op::function("ST_ANGLEFROMGML", &[GML], ANGLE, from_gml),
    Op::function("ST_ORDERINGCOMPARE", &[ANGLE, ANGLE], Ty::Integer, |a, _| {
        Ok(int(i64::from(angle_of(arg(a, 0)?)?.ordering_compare(angle_of(arg(a, 1)?)?))))
    }),
];

/// [§16.1.2 Description 5] ST_Angle(CHARACTER, DOUBLE PRECISION).
fn with_units(args: &[Value], _: Systems<'_>) -> Result<Value> {
    Angle::with_units(&text(args, 0)?, double(args, 1)?).map(angle_value).map_err(|f| fault(&f))
}

/// [§16.1.2 Description 9] ST_Angle(INTEGER, DOUBLE PRECISION).
fn degrees_minutes(args: &[Value], _: Systems<'_>) -> Result<Value> {
    Angle::from_degrees_minutes(integer(args, 0)?, double(args, 1)?).map(angle_value).map_err(|f| fault(&f))
}

/// [§16.1.2 Description 14] ST_Angle(INTEGER, INTEGER, DOUBLE PRECISION).
fn dms(args: &[Value], _: Systems<'_>) -> Result<Value> {
    Angle::from_dms(integer(args, 0)?, integer(args, 1)?, double(args, 2)?).map(angle_value).map_err(|f| fault(&f))
}

/// The coordinates of a point, none of the empty point.
pub(super) fn point_coordinates(g: &Geom) -> Result<Vec<f64>> {
    let Body::Point(coords) = &g.body else {
        return Err(fault(&Fault::new(Condition::PointNotWellFormed, "[§16.1.2 Description 17b] point value not well formed")));
    };
    Ok(coords.clone())
}

/// The points of a linestring, each its coordinates.
pub(super) fn line_points(g: &Geom) -> Result<Vec<Vec<f64>>> {
    let Body::Curve(Curve::LineString(coords)) = &g.body else {
        return Err(fault(&Fault::new(Condition::NotALine, "[§16.1.2 Description 23a] linestring is not a line")));
    };
    Ok(coords.chunks(g.dims.stride()).map(<[f64]>::to_vec).collect())
}

/// [§16.1.2 Description 17] ST_Angle(ST_Point, ST_Point, ST_Point).
fn at_point(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let at = point_coordinates(&geometry(args, 0)?)?;
    let a = point_coordinates(&geometry(args, 1)?)?;
    let b = point_coordinates(&geometry(args, 2)?)?;
    Angle::at_point(&at, &a, &b).map(angle_value).map_err(|f| fault(&f))
}

/// [§16.1.2 Description 23] ST_Angle(ST_LineString, ST_LineString).
fn between_lines(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let first = line_points(&geometry(args, 0)?)?;
    let second = line_points(&geometry(args, 1)?)?;
    Angle::between_lines(&views(&first), &views(&second)).map(angle_value).map_err(|f| fault(&f))
}

/// The points of a line as slices.
fn views(line: &[Vec<f64>]) -> Vec<&[f64]> {
    line.iter().map(Vec::as_slice).collect()
}

/// [§16.1.17] ST_AngleFromText, and ST_Angle(CHARACTER VARYING).
fn from_text(args: &[Value], _: Systems<'_>) -> Result<Value> {
    Angle::from_text(&text(args, 0)?).map(angle_value).map_err(|f| fault(&f))
}

/// [§16.1.18] ST_AngleFromGML, ST_Angle(CHARACTER LARGE OBJECT) and
/// ST_GMLToSQL.
fn from_gml(args: &[Value], _: Systems<'_>) -> Result<Value> {
    Angle::from_gml(&text(args, 0)?).map(angle_value).map_err(|f| fault(&f))
}

/// [§16.1.8] ST_String(INTEGER), ST_String() being ST_String(2).
fn string(v: &Value, places: i32) -> Result<Value> {
    angle_of(v)?.string(places).map(varchar).map_err(|f| fault(&f))
}
