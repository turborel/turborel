//! The curve productions of the well-known text representation,
//! ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67: point-list curves,
//! the parametric curves with their affine placements (§17.3.10), angles
//! (§16.1.21) and vectors (§17.2.22), and compound curves.

use super::cursor::{Cursor, mixed};
use super::lexer::{Token, invalid};
use crate::error::Result;
use crate::spatial::curves::fault::{Fault, fault};
use crate::spatial::curves::types::{
    Clothoid, Coord, Curve, Elliptical, Knot, Measures, Nurbs, NurbsPoint, Placement, Spiral, SpiralKind, Vector, without_m,
};
use crate::spatial::formats::tree::Kind;
use std::f64::consts::PI;

/// Gradians in a half turn.
const HALF_TURN_GRADIANS: f64 = 200.0;
/// The largest degree or multiplicity, one `<byte>` in binary.
const BYTE_MAX: i64 = 255;

/// A point with every coordinate NaN, an empty point in a placement.
pub const NAN_POINT: Coord = Coord { x: f64::NAN, y: f64::NAN, z: f64::NAN, m: f64::NAN };

/// A `<curve text>`: a bare `<linestring text body>` or a curve with its
/// keyword.
///
/// # Errors
///
/// As the curve's production's, and 2FF22 for a type that is no curve.
pub fn curve_text(c: &mut Cursor) -> Result<Curve> {
    if c.is_word("EMPTY") || c.peek() == Some(&Token::Open) {
        return Ok(Curve::LineString(c.points()?));
    }
    let kind = c.keyword(false)?;
    curve(c, kind)
}

/// The body of a curve of type `kind`.
///
/// # Errors
///
/// 2FF22 for a type that is no curve, 2FF25 for mixed dimensions and
/// 2FF84 for missing measures.
pub fn curve(c: &mut Cursor, kind: Kind) -> Result<Curve> {
    Ok(match kind {
        Kind::LineString => Curve::LineString(c.points()?),
        Kind::CircularString => Curve::CircularString(c.points()?),
        Kind::Circle => Curve::Circle(c.points()?),
        Kind::GeodesicString => Curve::GeodesicString(c.points()?),
        Kind::Elliptical => Curve::Elliptical(if c.opens()? { Some(elliptical(c)?) } else { None }),
        Kind::Nurbs => Curve::Nurbs(if c.opens()? { Some(nurbs(c)?) } else { None }),
        Kind::Clothoid => Curve::Clothoid(if c.opens()? { Some(clothoid(c)?) } else { None }),
        Kind::Spiral => Curve::Spiral(if c.opens()? { Some(spiral(c)?) } else { None }),
        Kind::CompoundCurve => Curve::Compound(c.list(curve_text)?),
        Kind::Point
        | Kind::CurvePolygon
        | Kind::Polygon
        | Kind::Triangle
        | Kind::Polyhedral
        | Kind::Tin
        | Kind::CompoundSurface
        | Kind::BRepSolid
        | Kind::GeometryCollection
        | Kind::MultiPoint
        | Kind::MultiCurve
        | Kind::MultiLineString
        | Kind::MultiSurface
        | Kind::MultiPolygon => return Err(invalid(&format!("{} is not a curve type", kind.keyword()))),
    })
}

/// `, KEYWORD number` for each of `words`.
fn fields(c: &mut Cursor, words: &[&str]) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(words.len());
    for word in words {
        c.expect(&Token::Comma)?;
        c.word(word)?;
        out.push(c.length()?);
    }
    Ok(out)
}

/// The optional `, STARTM n, ENDM n` and the closing parenthesis: given
/// exactly when the value has m (rule at and its kin).
fn measures(c: &mut Cursor) -> Result<Measures> {
    let given = c.take_comma();
    let found = if given {
        c.word("STARTM")?;
        let start = c.number()?;
        c.expect(&Token::Comma)?;
        c.word("ENDM")?;
        Some([start, c.number()?])
    } else {
        None
    };
    c.expect(&Token::Close)?;
    match (found.is_some(), c.dims.has_m()) {
        (true, true) | (false, false) => Ok(found),
        (false, true) => Err(fault(Fault::MissingMeasures, super::lexer::CLAUSE, "a measured parametric curve gives STARTM and ENDM")),
        (true, false) => Err(mixed("STARTM and ENDM need a measured value")),
    }
}

/// `REFERENCELOCATION AFFINEPLACEMENT [Z] <affineplacement text>`.
fn reference(c: &mut Cursor) -> Result<Placement> {
    c.word("REFERENCELOCATION")?;
    placement(c)
}

/// `AFFINEPLACEMENT [Z] <affineplacement text>` (§17.3.10); an empty one
/// has a NaN location and no directions.
///
/// # Errors
///
/// As its parts'.
pub fn placement(c: &mut Cursor) -> Result<Placement> {
    c.word("AFFINEPLACEMENT")?;
    c.just_z()?;
    if !c.opens()? {
        return Ok(Placement { location: NAN_POINT, directions: Vec::new() });
    }
    c.word("LOCATION")?;
    c.just_z()?;
    let location = c.point_text(without_m(c.dims))?.unwrap_or(NAN_POINT);
    c.expect(&Token::Comma)?;
    c.word("REFERENCEDIRECTIONS")?;
    let directions = c.list(vector)?;
    c.expect(&Token::Close)?;
    Ok(Placement { location, directions })
}

/// `VECTOR [Z] <vector text>` (§17.2.22); empty is NaN.
fn vector(c: &mut Cursor) -> Result<Vector> {
    c.word("VECTOR")?;
    c.just_z()?;
    let point = c.point_text(without_m(c.dims))?.unwrap_or(NAN_POINT);
    Ok(Vector { x: point.x, y: point.y, z: point.z })
}

/// `[ANGLE] DEGREES|GRADIANS|RADIANS ( n )` (§16.1.21), in radians.
fn angle(c: &mut Cursor) -> Result<f64> {
    c.take_word("ANGLE");
    let convert: fn(f64) -> f64 = if c.take_word("DEGREES") {
        f64::to_radians
    } else if c.take_word("GRADIANS") {
        |v| v * (PI / HALF_TURN_GRADIANS)
    } else {
        c.word("RADIANS")?;
        |v| v
    };
    c.expect(&Token::Open)?;
    let value = c.number()?;
    c.expect(&Token::Close)?;
    Ok(convert(value))
}

fn elliptical(c: &mut Cursor) -> Result<Elliptical> {
    let placement = reference(c)?;
    let axes = fields(c, &["UAXISLENGTH", "VAXISLENGTH"])?;
    let mut angles = [0.0_f64; 2];
    for (slot, word) in angles.iter_mut().zip(["STARTANGLE", "ENDANGLE"]) {
        c.expect(&Token::Comma)?;
        c.word(word)?;
        *slot = angle(c)?;
    }
    let [u_axis, v_axis] = <[f64; 2]>::try_from(axes).unwrap_or_default();
    let [start, end] = angles;
    Ok(Elliptical { placement, u_axis, v_axis, start, end, measures: measures(c)? })
}

fn clothoid(c: &mut Cursor) -> Result<Clothoid> {
    let placement = reference(c)?;
    let values = fields(c, &["SCALEFACTOR", "STARTDISTANCE", "ENDDISTANCE"])?;
    let [scale, start, end] = <[f64; 3]>::try_from(values).unwrap_or_default();
    Ok(Clothoid { placement, scale, start, end, measures: measures(c)? })
}

fn spiral(c: &mut Cursor) -> Result<Spiral> {
    let placement = reference(c)?;
    let values = fields(c, &["LENGTH", "STARTCURVATURE", "ENDCURVATURE"])?;
    let [length, start_curvature, end_curvature] = <[f64; 3]>::try_from(values).unwrap_or_default();
    c.expect(&Token::Comma)?;
    c.word("SPIRALTYPE")?;
    let kind = match c.token()? {
        Token::Word(w) => SpiralKind::named(&w),
        Token::Number(..) | Token::Quoted(_) | Token::Open | Token::Close | Token::Comma => None,
    };
    let kind =
        kind.ok_or_else(|| fault(Fault::InvalidArgument, "§4.2.12", "the spiral type is clothoid, bloss, biquadratic, sine or cosine"))?;
    Ok(Spiral { placement, length, start_curvature, end_curvature, kind, measures: measures(c)? })
}

fn nurbs(c: &mut Cursor) -> Result<Nurbs> {
    c.word("DEGREE")?;
    let degree = u32::try_from(c.integer(0, BYTE_MAX)?).unwrap_or_default();
    c.expect(&Token::Comma)?;
    c.word("CONTROLPOINTS")?;
    c.just_z()?;
    let points = c.list(nurbs_point)?;
    c.expect(&Token::Comma)?;
    c.word("KNOTS")?;
    let knots = c.list(knot)?;
    let rational = points.iter().any(|p| p.weight.is_some_and(|w| w.to_bits() != 1.0_f64.to_bits()));
    let points = points.into_iter().map(|p| NurbsPoint { weight: p.weight.filter(|_| rational), ..p }).collect();
    Ok(Nurbs { degree, points, knots, measures: measures(c)? })
}

/// `NURBSPOINT ( WEIGHTEDPOINT [Z] <point text>, WEIGHT n )`; a NURBS
/// point that is empty has no place in a curve.
fn nurbs_point(c: &mut Cursor) -> Result<NurbsPoint> {
    c.word("NURBSPOINT")?;
    if !c.opens()? {
        return Err(fault(Fault::InvalidArgument, "§17.4.1", "a NURBS curve's control point is not empty"));
    }
    c.word("WEIGHTEDPOINT")?;
    c.just_z()?;
    let point = c.point_text(without_m(c.dims))?.unwrap_or(NAN_POINT);
    c.expect(&Token::Comma)?;
    c.word("WEIGHT")?;
    let weight = c.number()?;
    c.expect(&Token::Close)?;
    Ok(NurbsPoint { point, weight: Some(weight) })
}

/// `KNOT ( VALUE n, MULTIPLICITY i )`.
fn knot(c: &mut Cursor) -> Result<Knot> {
    c.word("KNOT")?;
    if !c.opens()? {
        return Err(fault(Fault::InvalidArgument, "§17.5.1", "a NURBS curve's knot is not empty"));
    }
    c.word("VALUE")?;
    let value = c.number()?;
    c.expect(&Token::Comma)?;
    c.word("MULTIPLICITY")?;
    let multiplicity = u32::try_from(c.integer(1, BYTE_MAX)?).unwrap_or_default();
    c.expect(&Token::Close)?;
    Ok(Knot { value, multiplicity })
}
