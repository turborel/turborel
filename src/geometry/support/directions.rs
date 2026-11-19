//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §16.2] The routines of
//! ST_Direction, each the engine's (`spatial::srs::direction`): the seven
//! constructors, ST_Radians, ST_AngleNAzimuth, the nine bearing and
//! azimuth strings, ST_AddAngle, ST_SubtractAngle, the text and GML
//! representations, ST_DirectionFrmTxt, ST_DirectionFrmGML and
//! ST_OrderingCompare [§16.2.21], the function the type is ordered by.

use super::Support::{Angle as A, Direction as D};
use super::angles::{line_points, point_coordinates};
use super::ops::{Op, Systems, Ty, arg, clob, double, fault, geometry, int, integer, text, varchar};
use super::values::{angle_of, angle_value, direction_of, direction_value};
use crate::error::Result;
use crate::geometry::model::Kind;
use crate::spatial::srs::direction::{Direction, Measure, Reference};
use crate::value::Value;

/// ST_MaxDirectionAsText, implementation-defined.
pub const MAX_DIRECTION_AS_TEXT: u32 = 64;
/// ST_MaxDirectionAsGML, implementation-defined.
pub const MAX_DIRECTION_AS_GML: u32 = 4096;
/// ST_MaxDirectionString, the engine's (`direction::MAX_DIRECTION_STRING`).
pub const MAX_DIRECTION_STRING: u32 = 64;

const DIRECTION: Ty = Ty::Of(D);
const ANGLE: Ty = Ty::Of(A);
const TEXT: Ty = Ty::Varchar(MAX_DIRECTION_AS_TEXT);
const GML: Ty = Ty::Clob(MAX_DIRECTION_AS_GML);
const STRING: Ty = Ty::Varchar(MAX_DIRECTION_STRING);
const POINT: Ty = Ty::Geom(Kind::Point);

/// The routines.
pub(super) static OPS: &[Op] = &[
    Op::constructor(D, &[Ty::Double], |a, _| direction(Direction::new(double(a, 0)?))),
    Op::constructor(D, &[Ty::Char1, ANGLE, Ty::Char1], |a, _| {
        direction(Direction::bearing(&text(a, 0)?, angle_of(arg(a, 1)?)?, &text(a, 2)?))
    }),
    Op::constructor(D, &[Ty::Char1, ANGLE], |a, _| direction(Direction::azimuth(&text(a, 0)?, angle_of(arg(a, 1)?)?))),
    Op::constructor(D, &[POINT, POINT], from_points),
    Op::constructor(D, &[Ty::Geom(Kind::LineString)], from_line),
    Op::constructor(D, &[TEXT], from_text),
    Op::constructor(D, &[GML], from_gml),
    Op::method("ST_RADIANS", &[DIRECTION], Ty::Double, |a, _| Ok(Value::Approx(direction_of(arg(a, 0)?)?.radians()))),
    Op::method("ST_ANGLENAZIMUTH", &[DIRECTION], ANGLE, |a, _| Ok(angle_value(direction_of(arg(a, 0)?)?.angle_nazimuth()))),
    Op::method("ST_ANGLENAZIMUTH", &[DIRECTION, ANGLE], DIRECTION, |a, _| direction(Direction::with_nazimuth(angle_of(arg(a, 1)?)?))),
    Op::method("ST_ASTEXT", &[DIRECTION], TEXT, |a, _| Ok(varchar(direction_of(arg(a, 0)?)?.as_text()))),
    Op::method("ST_GMLTOSQL", &[DIRECTION, GML], DIRECTION, |a, s| from_gml(a.get(1..).unwrap_or_default(), s)),
    Op::method("ST_ASGML", &[DIRECTION], GML, |a, _| Ok(clob(&direction_of(arg(a, 0)?)?.as_gml()))),
    Op::method("ST_RADIANBEARING", &[DIRECTION, Ty::Integer], STRING, |a, _| string(a, Reference::Bearing, Measure::Radians)),
    Op::method("ST_DEGREESBEARING", &[DIRECTION, Ty::Integer], STRING, |a, _| string(a, Reference::Bearing, Measure::Degrees)),
    Op::method("ST_DMSBEARING", &[DIRECTION, Ty::Integer], STRING, |a, _| string(a, Reference::Bearing, Measure::Dms)),
    Op::method("ST_RADIANNAZIMUTH", &[DIRECTION, Ty::Integer], STRING, |a, _| string(a, Reference::North, Measure::Radians)),
    Op::method("ST_DEGREESNAZIMUTH", &[DIRECTION, Ty::Integer], STRING, |a, _| string(a, Reference::North, Measure::Degrees)),
    Op::method("ST_DMSNAZIMUTH", &[DIRECTION, Ty::Integer], STRING, |a, _| string(a, Reference::North, Measure::Dms)),
    Op::method("ST_RADIANSAZIMUTH", &[DIRECTION, Ty::Integer], STRING, |a, _| string(a, Reference::South, Measure::Radians)),
    Op::method("ST_DEGREESSAZIMUTH", &[DIRECTION, Ty::Integer], STRING, |a, _| string(a, Reference::South, Measure::Degrees)),
    Op::method("ST_DMSSAZIMUTH", &[DIRECTION, Ty::Integer], STRING, |a, _| string(a, Reference::South, Measure::Dms)),
    Op::method("ST_ADDANGLE", &[DIRECTION, ANGLE], DIRECTION, |a, _| {
        Ok(direction_value(direction_of(arg(a, 0)?)?.add_angle(angle_of(arg(a, 1)?)?)))
    }),
    Op::method("ST_SUBTRACTANGLE", &[DIRECTION, ANGLE], DIRECTION, |a, _| {
        Ok(direction_value(direction_of(arg(a, 0)?)?.subtract_angle(angle_of(arg(a, 1)?)?)))
    }),
    Op::function("ST_DIRECTIONFRMTXT", &[TEXT], DIRECTION, from_text),
    Op::function("ST_DIRECTIONFRMGML", &[GML], DIRECTION, from_gml),
    Op::function("ST_ORDERINGCOMPARE", &[DIRECTION, DIRECTION], Ty::Integer, |a, _| {
        Ok(int(i64::from(direction_of(arg(a, 0)?)?.ordering_compare(direction_of(arg(a, 1)?)?))))
    }),
];

/// The value of a direction the engine made.
fn direction(made: std::result::Result<Direction, crate::spatial::srs::Fault>) -> Result<Value> {
    made.map(direction_value).map_err(|f| fault(&f))
}

/// [§16.2.2 Description 9] ST_Direction(ST_Point, ST_Point).
fn from_points(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let from = point_coordinates(&geometry(args, 0)?)?;
    let to = point_coordinates(&geometry(args, 1)?)?;
    direction(Direction::from_points(&from, &to))
}

/// [§16.2.2 Description 12] ST_Direction(ST_LineString).
fn from_line(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let points = line_points(&geometry(args, 0)?)?;
    direction(Direction::from_line(&points.iter().map(Vec::as_slice).collect::<Vec<&[f64]>>()))
}

/// [§16.2.19] ST_DirectionFrmTxt, and ST_Direction(CHARACTER VARYING).
fn from_text(args: &[Value], _: Systems<'_>) -> Result<Value> {
    direction(Direction::from_text(&text(args, 0)?))
}

/// [§16.2.20] ST_DirectionFrmGML, ST_Direction(CHARACTER LARGE OBJECT)
/// and ST_GMLToSQL.
fn from_gml(args: &[Value], _: Systems<'_>) -> Result<Value> {
    direction(Direction::from_gml(&text(args, 0)?))
}

/// [§16.2.8 to §16.2.16] A bearing or azimuth string of SELF, of the
/// decimal digits the second argument says.
fn string(args: &[Value], reference: Reference, measure: Measure) -> Result<Value> {
    let d = direction_of(arg(args, 0)?)?;
    d.string(reference, measure, integer(args, 1)?).map(varchar).map_err(|f| fault(&f))
}
