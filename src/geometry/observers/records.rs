//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.10, §7.3.13, §7.3.14,
//! §7.4.7, §7.6.3, §7.6.6, §7.6.7, §7.7.4, §7.7.5, §7.8.3, §7.9.3] The
//! curve observers whose results are values of the types of §16 and §17
//! (`geometry::support`): the reference location of elliptical curves,
//! clothoids and spirals, an ST_AffinePlacement whose reference
//! directions are its axis vectors; the control points and knots of a
//! NURBS curve; the start and end angles of an arc and of an elliptical
//! curve, ST_Angle values; and the bulge normal of an arc and the normal
//! of a circle, ST_Vector values.

use super::{Observer, Returns, Unit, Warned};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Dims, Geom, Kind};
use crate::geometry::representations::spatial;
use crate::geometry::support::{self, Support};
use crate::spatial::curves::circular;
use crate::spatial::curves::types::{Coord, Curve, Placement};
use crate::spatial::formats::model::to_geom;
use crate::spatial::formats::tree::{Body, Geometry};
use crate::value::Value;

/// A curve observer of a §16 or §17 result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordMethod {
    /// [§7.6.3, §7.8.3, §7.9.3] ST_RefLocation().
    RefLocation,
    /// [§7.7.4] ST_ControlPoints().
    ControlPoints,
    /// [§7.7.5] ST_Knots().
    Knots,
    /// [§7.3.13, §7.6.6] ST_StartAngle().
    StartAngle,
    /// [§7.3.14, §7.6.7] ST_EndAngle().
    EndAngle,
    /// [§7.3.10] ST_BulgeNormal().
    BulgeNormal,
    /// [§7.4.7] ST_Normal().
    Normal,
}

/// The observers.
pub const RECORD_METHODS: [RecordMethod; 7] = [
    RecordMethod::RefLocation,
    RecordMethod::ControlPoints,
    RecordMethod::Knots,
    RecordMethod::StartAngle,
    RecordMethod::EndAngle,
    RecordMethod::BulgeNormal,
    RecordMethod::Normal,
];

/// The dimensions of a point of a value of `dims`: z if it has z, no m
/// [§17.3.1 Description 6].
const fn without_m(dims: Dims) -> Dims {
    if dims.has_z() { Dims::Xyz } else { Dims::Xy }
}

/// The payload of the point `at` of dimensions `dims` in `g`'s system.
fn point(g: &Geom, dims: Dims, at: Coord) -> Result<Vec<u8>> {
    Ok(to_geom(&Geometry { dims, body: Body::Point(Some(at)) }, g.srid)?.encode())
}

/// The ST_Vector of `v` in `g`'s system, with z if `g` has z.
fn vector(g: &Geom, [x, y, z]: [f64; 3]) -> Value {
    support::vector_in(x, y, g.dims.has_z().then_some(z), g.srid)
}

/// [§17.3] The ST_AffinePlacement of a curve's placement.
fn placement(g: &Geom, p: &Placement) -> Result<Value> {
    let directions: Vec<Value> = p.directions.iter().map(|d| vector(g, d.space())).collect();
    support::placement_value(point(g, without_m(g.dims), p.location)?, &directions)
}

impl RecordMethod {
    /// The arc of a circular string of one arc, or the circle's points.
    fn arc(c: &Curve) -> Result<Option<[Coord; 3]>> {
        match c {
            Curve::CircularString(points) => circular::single(points),
            Curve::Circle(points) => Ok(<[Coord; 3]>::try_from(points.as_slice()).ok()),
            Curve::LineString(_)
            | Curve::GeodesicString(_)
            | Curve::Elliptical(_)
            | Curve::Nurbs(_)
            | Curve::Clothoid(_)
            | Curve::Spiral(_)
            | Curve::Compound(_) => Ok(None),
        }
    }

    /// Its result of a NURBS curve's control points or knots.
    fn nurbs(self, g: &Geom, c: &Curve) -> Result<Value> {
        let Curve::Nurbs(n) = c else { return Err(self.refused()) };
        let Some(n) = n else { return Ok(Value::Null) };
        let all = if self == Self::Knots {
            n.knots.iter().map(|k| support::knot_value(k.value, i32::try_from(k.multiplicity).unwrap_or(i32::MAX))).collect()
        } else {
            n.points.iter().map(|p| Ok(support::nurbs_point_value(point(g, g.dims, p.point)?, p.weight))).collect::<Result<Vec<_>>>()?
        };
        Ok(Value::Array(all.into()))
    }

    /// Its result of an arc's or a circle's points.
    fn of_arc(self, g: &Geom, c: &Curve) -> Result<Value> {
        let Some(arc) = Self::arc(c)? else { return Ok(Value::Null) };
        Ok(match self {
            Self::StartAngle | Self::EndAngle => {
                let Some((start, end)) = circular::angles(arc) else { return Ok(Value::Null) };
                support::angle_value(if self == Self::StartAngle { start } else { end })
            }
            Self::BulgeNormal => vector(g, circular::bulge(arc, g.dims.has_z()).1),
            Self::Normal => circular::normal(arc).map_or(Value::Null, |n| vector(g, n)),
            Self::RefLocation | Self::ControlPoints | Self::Knots => return Err(self.refused()),
        })
    }

    fn refused(self) -> crate::error::SqlError {
        spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§7", &format!("{} takes no curve of this type", self.name()))
    }
}

impl Observer for RecordMethod {
    fn name(self) -> &'static str {
        match self {
            Self::RefLocation => "ST_REFLOCATION",
            Self::ControlPoints => "ST_CONTROLPOINTS",
            Self::Knots => "ST_KNOTS",
            Self::StartAngle => "ST_STARTANGLE",
            Self::EndAngle => "ST_ENDANGLE",
            Self::BulgeNormal => "ST_BULGENORMAL",
            Self::Normal => "ST_NORMAL",
        }
    }

    fn parameters(self) -> &'static [super::Parameter] {
        &[]
    }

    fn returns(self, _: Kind) -> Returns {
        match self {
            Self::RefLocation => Returns::Support(Support::AffinePlacement),
            Self::ControlPoints => Returns::Supports(Support::NurbsPoint),
            Self::Knots => Returns::Supports(Support::Knot),
            Self::StartAngle | Self::EndAngle => Returns::Support(Support::Angle),
            Self::BulgeNormal | Self::Normal => Returns::Support(Support::Vector),
        }
    }

    fn warns(self) -> bool {
        false
    }

    fn unit(self) -> Unit {
        Unit::None
    }

    fn of(self, g: &Geom, value: &Geometry, _: &[Value]) -> Result<Warned> {
        let Body::Curve(c) = &value.body else { return Err(self.refused()) };
        let result = match (self, c) {
            (Self::RefLocation, Curve::Elliptical(e)) => e.as_ref().map(|e| placement(g, &e.placement)).transpose()?,
            (Self::RefLocation, Curve::Clothoid(k)) => k.as_ref().map(|k| placement(g, &k.placement)).transpose()?,
            (Self::RefLocation, Curve::Spiral(s)) => s.as_ref().map(|s| placement(g, &s.placement)).transpose()?,
            (Self::StartAngle, Curve::Elliptical(e)) => e.as_ref().map(|e| support::angle_value(e.start)),
            (Self::EndAngle, Curve::Elliptical(e)) => e.as_ref().map(|e| support::angle_value(e.end)),
            (Self::ControlPoints | Self::Knots, _) => Some(self.nurbs(g, c)?),
            (Self::StartAngle | Self::EndAngle | Self::BulgeNormal | Self::Normal, _) => Some(self.of_arc(g, c)?),
            (Self::RefLocation, _) => return Err(self.refused()),
        };
        Ok((result.unwrap_or(Value::Null), None))
    }
}
