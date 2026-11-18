//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.6 to §7.9] The mutators of
//! the curves given by parameters: the reference location, axis lengths
//! and angles of an elliptical curve; the control points and knots of a
//! NURBS curve; the scale factor and distances of a clothoid; the length,
//! curvatures and type of a spiral; and the start and end measures of
//! each. A length given with a unit is converted to the unit of SELF's
//! spatial reference system [§7.6.4 Description 4b].

use super::{Systems, invalid};
use crate::error::{Result, SqlState};
use crate::geometry::model::{
    Body, Clothoid, Curve, Dims, Elliptical, Geom, Knot, Measures, Nurbs, NurbsPoint, Placement, Spiral, SpiralType,
};
use crate::geometry::observers::signature;
use crate::geometry::representations::spatial;
use crate::geometry::support::{self, VectorParts};
use crate::value::Value;

/// The reference directions of a placement [§7.6.3 Description 4d].
const DIRECTIONS: usize = 2;

/// An attribute of a curve given by parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Attribute {
    RefLocation,
    UAxisLength,
    VAxisLength,
    StartAngle,
    EndAngle,
    StartM,
    EndM,
    ControlPoints,
    Knots,
    ScaleFactor,
    StartDistance,
    EndDistance,
    Length,
    StartCurvature,
    EndCurvature,
    SpiralType,
}

/// The parameters of a curve that is not empty.
enum Params<'a> {
    Elliptical(&'a mut Elliptical),
    Nurbs(&'a mut Nurbs),
    Clothoid(&'a mut Clothoid),
    Spiral(&'a mut Spiral),
}

/// The parameters of `c`.
fn params<'a>(c: &'a mut Curve, name: &str) -> Result<Params<'a>> {
    match c {
        Curve::Elliptical(Some(e)) => Ok(Params::Elliptical(e)),
        Curve::Nurbs(Some(n)) => Ok(Params::Nurbs(n)),
        Curve::Clothoid(Some(k)) => Ok(Params::Clothoid(k)),
        Curve::Spiral(Some(s)) => Ok(Params::Spiral(s)),
        Curve::Elliptical(None) | Curve::Nurbs(None) | Curve::Clothoid(None) | Curve::Spiral(None) => {
            Err(invalid("§7.6 to §7.9", &format!("{name} of an empty curve given by parameters; its constructor gives them all")))
        }
        Curve::LineString(_) | Curve::CircularString(_) | Curve::Circle(_) | Curve::GeodesicString(_) | Curve::Compound(_) => {
            Err(signature(name))
        }
    }
}

impl Params<'_> {
    fn measures(&mut self) -> &mut Option<Measures> {
        match self {
            Self::Elliptical(e) => &mut e.measures,
            Self::Nurbs(n) => &mut n.measures,
            Self::Clothoid(k) => &mut k.measures,
            Self::Spiral(s) => &mut s.measures,
        }
    }

    fn placement(&mut self) -> Option<&mut Placement> {
        match self {
            Self::Elliptical(e) => Some(&mut e.placement),
            Self::Clothoid(k) => Some(&mut k.placement),
            Self::Spiral(s) => Some(&mut s.placement),
            Self::Nurbs(_) => None,
        }
    }
}

impl<'a> Params<'a> {
    fn elliptical(self) -> Option<&'a mut Elliptical> {
        if let Self::Elliptical(e) = self { Some(e) } else { None }
    }

    fn clothoid(self) -> Option<&'a mut Clothoid> {
        if let Self::Clothoid(k) = self { Some(k) } else { None }
    }

    fn spiral(self) -> Option<&'a mut Spiral> {
        if let Self::Spiral(s) = self { Some(s) } else { None }
    }
}

impl Attribute {
    /// The number the attribute is of the parameters `p`, if it is one.
    fn number(self, p: Params<'_>) -> Option<&mut f64> {
        match self {
            Self::UAxisLength => p.elliptical().map(|e| &mut e.u_axis),
            Self::VAxisLength => p.elliptical().map(|e| &mut e.v_axis),
            Self::StartAngle => p.elliptical().map(|e| &mut e.start_angle),
            Self::EndAngle => p.elliptical().map(|e| &mut e.end_angle),
            Self::ScaleFactor => p.clothoid().map(|k| &mut k.scale_factor),
            Self::StartDistance => p.clothoid().map(|k| &mut k.start_distance),
            Self::EndDistance => p.clothoid().map(|k| &mut k.end_distance),
            Self::Length => p.spiral().map(|s| &mut s.length),
            Self::StartCurvature => p.spiral().map(|s| &mut s.start_curvature),
            Self::EndCurvature => p.spiral().map(|s| &mut s.end_curvature),
            Self::RefLocation | Self::StartM | Self::EndM | Self::ControlPoints | Self::Knots | Self::SpiralType => None,
        }
    }
}

/// The number `arg`, an angle's radians of an ST_Angle, a length in the
/// unit `unit` converted to `g`'s.
fn number_of(a: Attribute, g: &Geom, arg: &Value, unit: Option<(&str, Systems<'_>)>, name: &str) -> Result<f64> {
    if matches!(a, Attribute::StartAngle | Attribute::EndAngle) {
        return support::angle_radians(arg);
    }
    let Value::Approx(x) = arg else { return Err(signature(name)) };
    let Some((unit, systems)) = unit else { return Ok(*x) };
    crate::geometry::routines::length_of(*x, (g.srid, systems), unit)
}

/// [§7.6.8, §7.6.9] The measures of `p` with the start or the end set to
/// `arg`, or none if it is null; and the dimensions they give.
fn measured(a: Attribute, mut p: Params<'_>, dims: Dims, arg: &Value) -> Dims {
    let Value::Approx(x) = arg else {
        *p.measures() = None;
        return dims.with_m(false);
    };
    let m = p.measures().get_or_insert(Measures { start: *x, end: *x });
    if a == Attribute::StartM {
        m.start = *x;
    } else {
        m.end = *x;
    }
    dims.with_m(true)
}

/// The point of the ST_Point payload `bytes`, and whether it has z.
pub(super) fn point_of(bytes: Option<&[u8]>, clause: &str) -> Result<(Vec<f64>, bool)> {
    let p = Geom::decode(bytes.ok_or_else(|| invalid(clause, "a point is not null"))?)?;
    let Body::Point(coords) = &p.body else { return Err(invalid(clause, "an ST_Point")) };
    let planar = coords.get(..p.dims.planar()).ok_or_else(|| invalid(clause, "a point is not empty"))?;
    Ok((planar.to_vec(), p.dims.has_z()))
}

/// [§7.6.2, §7.6.3 Description 4c, 4d] The placement of the
/// ST_AffinePlacement `arg`, and whether it is 3D: its directions are
/// two, of its location's dimension.
pub(super) fn placement_of(arg: &Value) -> Result<(Placement, bool)> {
    const CLAUSE: &str = "§7.6.3 Description 4";
    let (location, directions) = support::placement_parts(arg)?;
    let (location, z) = point_of(location.as_deref(), CLAUSE)?;
    let directions: Vec<VectorParts> = directions.unwrap_or_default().into_iter().flatten().collect();
    if directions.iter().any(|d| d.z.is_some() != z) {
        return Err(mixed_is3d());
    }
    if directions.len() != DIRECTIONS {
        return Err(spatial(SqlState::SPATIAL_INCORRECT_NUMBER_OF_VECTORS, "§7.6.3 Description 4d", "incorrect number of vectors"));
    }
    let directions = directions.iter().flat_map(|d| [Some(d.x), Some(d.y), d.z]).flatten().collect();
    Ok((Placement { location, directions }, z))
}

/// 2FF96 mixed Is3D.
fn mixed_is3d() -> crate::error::SqlError {
    spatial(SqlState::SPATIAL_MIXED_IS3D, "§7.6.3 Description 4c", "mixed Is3D")
}

/// [§7.6.3 Description 4] The placement of the ST_AffinePlacement `arg`
/// for a curve of `dims`.
fn placement(arg: &Value, dims: Dims) -> Result<Placement> {
    let (p, z) = placement_of(arg)?;
    if z != dims.has_z() {
        return Err(mixed_is3d());
    }
    Ok(p)
}

/// The elements of the ARRAY `arg`, none null.
fn items<'a>(arg: &'a Value, name: &str) -> Result<&'a [Value]> {
    let Value::Array(items) = arg else { return Err(signature(name)) };
    if items.iter().any(Value::is_null) {
        return Err(spatial(SqlState::SPATIAL_ELEMENT_IS_A_NULL_VALUE, "§7.7.4 Description 4a", "element is a null value"));
    }
    Ok(items)
}

/// [§7.7.4] The control points of the ST_NURBSPoint ARRAY `arg`, and
/// whether they have z.
pub(super) fn control_points(arg: &Value, name: &str) -> Result<(Vec<NurbsPoint>, bool)> {
    const CLAUSE: &str = "§7.7.4 Description 4";
    let mut out = Vec::new();
    let mut z = None;
    for item in items(arg, name)? {
        let (bytes, weight) = support::nurbs_point_parts(item)?;
        let (point, has_z) = point_of(bytes.as_deref(), CLAUSE)?;
        if *z.get_or_insert(has_z) != has_z {
            return Err(spatial(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, CLAUSE, "mixed coordinate dimensions"));
        }
        out.push(NurbsPoint { point, weight });
    }
    Ok((out, z.unwrap_or_default()))
}

/// [§7.7.5] The knots of the ST_Knot ARRAY `arg`.
pub(super) fn knots(arg: &Value, name: &str) -> Result<Vec<Knot>> {
    items(arg, name)?
        .iter()
        .map(|item| {
            let (value, multiplicity) = support::knot_parts(item)?;
            let multiplicity = multiplicity.and_then(|m| u8::try_from(m).ok());
            let (Some(value), Some(multiplicity)) = (value, multiplicity) else {
                return Err(invalid("§7.7.5 Description 4", "a knot has a value and a multiplicity"));
            };
            Ok(Knot { value, multiplicity })
        })
        .collect()
}

/// `g`, a curve given by parameters, with the attribute `a` set to `arg`,
/// a length's in the unit `unit` if given.
pub(super) fn replace(attribute: Attribute, name: &str, g: &Geom, arg: &Value, unit: Option<(&str, Systems<'_>)>) -> Result<Geom> {
    let Body::Curve(given) = &g.body else { return Err(signature(name)) };
    let mut curve = given.clone();
    let mut dims = g.dims;
    match attribute {
        Attribute::StartM | Attribute::EndM => dims = measured(attribute, params(&mut curve, name)?, g.dims, arg),
        Attribute::RefLocation => {
            let p = placement(arg, g.dims)?;
            *params(&mut curve, name)?.placement().ok_or_else(|| signature(name))? = p;
        }
        Attribute::ControlPoints => {
            let (points, z) = control_points(arg, name)?;
            let Params::Nurbs(n) = params(&mut curve, name)? else { return Err(signature(name)) };
            n.points = points;
            dims = g.dims.with_z(z);
        }
        Attribute::Knots => {
            let all = knots(arg, name)?;
            let Params::Nurbs(n) = params(&mut curve, name)? else { return Err(signature(name)) };
            n.knots = all;
        }
        Attribute::SpiralType => {
            let Value::Char(text) = arg else { return Err(signature(name)) };
            let kind = SpiralType::named(text.trim_end()).ok_or_else(|| invalid("§7.9.7 Description 4", "an unknown spiral type"))?;
            let Params::Spiral(s) = params(&mut curve, name)? else { return Err(signature(name)) };
            s.spiral_type = kind;
        }
        Attribute::UAxisLength
        | Attribute::VAxisLength
        | Attribute::StartAngle
        | Attribute::EndAngle
        | Attribute::ScaleFactor
        | Attribute::StartDistance
        | Attribute::EndDistance
        | Attribute::Length
        | Attribute::StartCurvature
        | Attribute::EndCurvature => {
            let x = number_of(attribute, g, arg, unit, name)?;
            *attribute.number(params(&mut curve, name)?).ok_or_else(|| signature(name))? = x;
        }
    }
    Geom::new(g.srid, dims, Body::Curve(curve))
}
