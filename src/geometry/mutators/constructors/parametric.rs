//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.6.2, §7.7.2, §7.8.2,
//! §7.9.2] The constructors of the curves given by parameters: an
//! elliptical curve, a NURBS curve, a clothoid and a spiral, each of its
//! parameters and, if given, its start and end measures, the unit of its
//! lengths and its SRID. A form defined by another passes the null value
//! for what it lacks and is null-call, null of a null argument; the form
//! that defines it raises 2FF03 of a null parameter and 2FF84 of one
//! measure without the other. Lengths given with a unit are converted to
//! the unit of the reference system [§7.6.2 Description 23].

use super::Param;
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Clothoid, Curve, Dims, Elliptical, Geom, Kind, Measures, Nurbs, Placement, Spiral, SpiralType};
use crate::geometry::mutators::parameters::{control_points, knots, placement_of};
use crate::geometry::mutators::{Systems, invalid, null_argument};
use crate::geometry::observers::{signature, whole};
use crate::geometry::representations::spatial;
use crate::geometry::support;
use crate::value::Value;

/// Whether `kind` is a curve given by parameters.
pub(super) const fn takes(kind: Kind) -> bool {
    matches!(kind, Kind::EllipticalCurve | Kind::NurbsCurve | Kind::Clothoid | Kind::SpiralCurve)
}

/// The arguments of a form, by their parts.
struct Given<'a> {
    /// The curve's own parameters.
    own: &'a [Value],
    /// The start and end measures, if the form takes them.
    measures: Option<(&'a Value, &'a Value)>,
    /// The unit's name, if given.
    unit: Option<&'a Value>,
    /// The SRID, if given.
    srid: Option<&'a Value>,
}

/// The parameters of `kind`'s own, before its suffix.
const fn own_count(kind: Kind) -> usize {
    match kind {
        Kind::NurbsCurve => 3,
        Kind::Clothoid => 4,
        Kind::EllipticalCurve | Kind::SpiralCurve => 5,
        Kind::Point
        | Kind::Geometry
        | Kind::Curve
        | Kind::LineString
        | Kind::CircularString
        | Kind::Circle
        | Kind::GeodesicString
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
        | Kind::GeomCollection
        | Kind::MultiPoint
        | Kind::MultiCurve
        | Kind::MultiLineString
        | Kind::MultiSurface
        | Kind::MultiPolygon => 0,
    }
}

/// `args` of the form `form` by their parts.
fn parts<'a>(kind: Kind, form: &[Param], args: &'a [Value]) -> Result<Given<'a>> {
    let at = own_count(kind);
    let (own, rest) = args.split_at_checked(at).ok_or_else(|| signature(kind.type_name()))?;
    let suffix = form.get(at..).unwrap_or_default();
    let with_measures = suffix.starts_with(&[Param::Double, Param::Double]);
    let skip = if with_measures { 2 } else { 0 };
    let after: Vec<(&Param, &Value)> = suffix.iter().zip(rest).skip(skip).collect();
    let pick = |p: Param| after.iter().find(|(q, _)| **q == p).map(|(_, v)| *v);
    let measures = if with_measures { rest.first().zip(rest.get(1)) } else { None };
    Ok(Given { own, measures, unit: pick(Param::Unit), srid: pick(Param::Integer) })
}

/// The DOUBLE PRECISION `v`.
fn number(v: Option<&Value>, name: &str) -> Result<f64> {
    if let Some(Value::Approx(x)) = v { Ok(*x) } else { Err(signature(name)) }
}

/// The SRID, 0 if not given.
fn srid_of(g: &Given<'_>, name: &str) -> Result<i32> {
    g.srid.map_or(Ok(0), |v| i32::try_from(whole(Some(v), name)?).map_err(|_| signature(name)))
}

/// The measures, both or none [§7.6.2 Description 24c].
fn measures_of(g: &Given<'_>) -> Result<Option<Measures>> {
    let Some((start, end)) = g.measures else { return Ok(None) };
    if start.is_null() && end.is_null() {
        return Ok(None);
    }
    let (Value::Approx(start), Value::Approx(end)) = (start, end) else {
        return Err(spatial(SqlState::SPATIAL_MISSING_MEASURE_VALUES, "§7.6.2 Description 24c", "missing measure value(s)"));
    };
    Ok(Some(Measures { start: *start, end: *end }))
}

/// The curve of the parameters given by `g`, a length `x` in its unit.
struct Build<'a> {
    kind: Kind,
    given: Given<'a>,
    srid: i32,
    systems: Systems<'a>,
}

impl Build<'_> {
    fn name(&self) -> &'static str {
        self.kind.type_name()
    }

    /// The own parameter at `at`, a number.
    fn number(&self, at: usize) -> Result<f64> {
        number(self.given.own.get(at), self.name())
    }

    /// The own parameter at `at`, a length in the given unit.
    fn length(&self, at: usize) -> Result<f64> {
        let x = self.number(at)?;
        let Some(unit) = self.given.unit else { return Ok(x) };
        let Value::Char(unit) = unit else { return Err(signature(self.name())) };
        crate::geometry::routines::length_of(x, (self.srid, self.systems), unit)
    }

    /// The own parameter at `at`, an angle's radians.
    fn angle(&self, at: usize) -> Result<f64> {
        support::angle_radians(self.given.own.get(at).ok_or_else(|| signature(self.name()))?)
    }

    /// The placement, first, and whether it is 3D.
    fn placement(&self) -> Result<(Placement, bool)> {
        placement_of(self.given.own.first().ok_or_else(|| signature(self.name()))?)
    }

    /// [§7.7.2] A NURBS curve of its degree, control points and knots.
    fn nurbs(&self, measures: Option<Measures>) -> Result<(Curve, bool)> {
        let degree = whole(self.given.own.first(), self.name())?;
        let degree = u8::try_from(degree).map_err(|_| invalid("§7.7.2", "a degree from 1 to 255"))?;
        let empty = Value::Null;
        let (points, z) = control_points(self.given.own.get(1).unwrap_or(&empty), self.name())?;
        let knots = knots(self.given.own.get(2).unwrap_or(&empty), self.name())?;
        Ok((Curve::Nurbs(Some(Nurbs { degree, points, knots, measures })), z))
    }

    fn curve(&self, measures: Option<Measures>) -> Result<(Curve, bool)> {
        Ok(match self.kind {
            Kind::EllipticalCurve => {
                let (placement, z) = self.placement()?;
                let (u_axis, v_axis) = (self.length(1)?, self.length(2)?);
                let (start_angle, end_angle) = (self.angle(3)?, self.angle(4)?);
                (Curve::Elliptical(Some(Elliptical { placement, u_axis, v_axis, start_angle, end_angle, measures })), z)
            }
            Kind::Clothoid => {
                let (placement, z) = self.placement()?;
                let (scale_factor, start_distance, end_distance) = (self.number(1)?, self.length(2)?, self.length(3)?);
                (Curve::Clothoid(Some(Clothoid { placement, scale_factor, start_distance, end_distance, measures })), z)
            }
            Kind::SpiralCurve => {
                let (placement, z) = self.placement()?;
                let (length, start_curvature, end_curvature) = (self.length(1)?, self.number(2)?, self.number(3)?);
                let Some(Value::Char(name)) = self.given.own.get(4) else { return Err(signature(self.name())) };
                let spiral_type = SpiralType::named(name.trim_end()).ok_or_else(|| invalid("§7.9.2", "an unknown spiral type"))?;
                (Curve::Spiral(Some(Spiral { placement, length, start_curvature, end_curvature, spiral_type, measures })), z)
            }
            Kind::NurbsCurve => self.nurbs(measures)?,
            Kind::Point
            | Kind::Geometry
            | Kind::Curve
            | Kind::LineString
            | Kind::CircularString
            | Kind::Circle
            | Kind::GeodesicString
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
            | Kind::GeomCollection
            | Kind::MultiPoint
            | Kind::MultiCurve
            | Kind::MultiLineString
            | Kind::MultiSurface
            | Kind::MultiPolygon => return Err(signature(self.name())),
        })
    }
}

/// [§7.6.2 to §7.9.2] The curve of `kind` the form `form` of `args`
/// gives, a unit's lengths in the reference systems `systems`.
pub(super) fn build(kind: Kind, form: &[Param], args: &[Value], systems: Systems<'_>) -> Result<Value> {
    let given = parts(kind, form, args)?;
    let defining = given.measures.is_some() && given.srid.is_some();
    let own_null = given.own.iter().chain(given.unit).chain(given.srid).any(Value::is_null);
    if !defining && args.iter().any(Value::is_null) {
        return Ok(Value::Null);
    }
    if own_null {
        return Err(null_argument(kind.type_name()));
    }
    let measures = measures_of(&given)?;
    let srid = srid_of(&given, kind.type_name())?;
    let b = Build { kind, given, srid, systems };
    let (curve, z) = b.curve(measures)?;
    Ok(Geom::new(srid, Dims::Xy.with_z(z).with_m(measures.is_some()), Body::Curve(curve))?.to_value())
}

/// [§6.1.2 Description 9 to 20] A point of x, y, z and m, an SRID last
/// if given: the empty point if x and y are null, of no z then. The
/// forms of z and an SRID are not null-call, and the one without m
/// takes no null z of a point that is not empty [Description 16c].
pub(super) fn point(form: &[Param], args: &[Value]) -> Result<Value> {
    const CLAUSE: &str = "§6.1.2";
    const WITH_Z: usize = 4;
    let defining = form.len() >= WITH_Z && form.last() == Some(&Param::Integer);
    let (coordinates, srid) = if defining { args.split_at(args.len().saturating_sub(1)) } else { (args, &[][..]) };
    if srid.iter().any(Value::is_null) || (!defining && args.iter().any(Value::is_null)) {
        return Ok(Value::Null);
    }
    let srid = srid.first().map_or(Ok(0_i64), |v| whole(Some(v), "ST_POINT"))?;
    let srid = i32::try_from(srid).map_err(|_| signature("ST_POINT"))?;
    let at = |i: usize| if let Some(Value::Approx(x)) = coordinates.get(i) { Some(*x) } else { None };
    let (x, y, z, m) = (at(0), at(1), at(2), at(3));
    let point = match (x, y) {
        (Some(x), Some(y)) => {
            if form.len() == WITH_Z && z.is_none() {
                return Err(null_argument("ST_POINT"));
            }
            [Some(x), Some(y), z, m].into_iter().flatten().collect()
        }
        (None, None) => {
            if z.is_some() {
                return Err(spatial(SqlState::SPATIAL_NOT_AN_EMPTY_SET, CLAUSE, "not an empty set"));
            }
            Vec::new()
        }
        (Some(_), None) | (None, Some(_)) => return Err(null_argument("ST_POINT")),
    };
    Ok(Geom::new(srid, Dims::Xy.with_z(z.is_some()).with_m(m.is_some()), Body::Point(point))?.to_value())
}
