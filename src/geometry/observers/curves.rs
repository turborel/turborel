//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7] The curve observers: those
//! of every curve (§7.1), the point lists of line, circular and geodesic
//! strings and circles (§7.2 to §7.5), the arcs' (§7.3, §7.4), the
//! parameters of the parametric curves (§7.6 to §7.9) and the parts of a
//! compound curve (§7.10).

use super::{Observer, Parameter, Returns, Unit, Warned, array, double, found, int, number, part, signature, truth, whole};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Dims, Geom, Kind};
use crate::geometry::representations::spatial;
use crate::spatial::curves::circular;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::linearize::Tolerance;
use crate::spatial::curves::observe::{self, Foot};
use crate::spatial::curves::piece::Metric;
use crate::spatial::curves::types::{Coord, Curve};
use crate::spatial::formats::model::from_geom;
use crate::spatial::formats::tree::{Body, Collection, Geometry};
use crate::value::Value;

/// The curve of `value`, which a method of a curve type takes.
fn curve<'v>(value: &'v Geometry, name: &str) -> Result<&'v Curve> {
    let Body::Curve(c) = &value.body else {
        return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§7.1", &format!("{name} takes a curve")));
    };
    Ok(c)
}

/// The point of the ST_Point argument, none when it is empty.
fn point_argument(v: Option<&Value>, name: &str) -> Result<Option<Coord>> {
    let g = Geom::from_value(v.ok_or_else(|| signature(name))?)?;
    let Body::Point(p) = from_geom(&g)?.body else { return Err(signature(name)) };
    Ok(p)
}

/// [§7.1] The methods of ST_Curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveMethod {
    /// [§7.1.4] ST_StartPoint().
    StartPoint,
    /// [§7.1.5] ST_EndPoint().
    EndPoint,
    /// [§7.1.6] ST_IsClosed().
    IsClosed,
    /// [§7.1.10] ST_CurveToLine().
    CurveToLine,
    /// [§7.1.11] ST_DistanceToPoint(ST_Point).
    DistanceToPoint,
    /// [§7.1.13] ST_PointAtDistance(DOUBLE PRECISION).
    PointAtDistance,
    /// [§7.1.15] ST_PerpPoints(ST_Point).
    PerpPoints,
}

/// The methods of ST_Curve.
pub const CURVE_METHODS: [CurveMethod; 7] = [
    CurveMethod::StartPoint,
    CurveMethod::EndPoint,
    CurveMethod::IsClosed,
    CurveMethod::CurveToLine,
    CurveMethod::DistanceToPoint,
    CurveMethod::PointAtDistance,
    CurveMethod::PerpPoints,
];

impl Observer for CurveMethod {
    fn name(self) -> &'static str {
        match self {
            Self::StartPoint => "ST_STARTPOINT",
            Self::EndPoint => "ST_ENDPOINT",
            Self::IsClosed => "ST_ISCLOSED",
            Self::CurveToLine => "ST_CURVETOLINE",
            Self::DistanceToPoint => "ST_DISTANCETOPOINT",
            Self::PointAtDistance => "ST_POINTATDISTANCE",
            Self::PerpPoints => "ST_PERPPOINTS",
        }
    }

    fn parameters(self) -> &'static [Parameter] {
        match self {
            Self::DistanceToPoint | Self::PerpPoints => &[Parameter::Point],
            Self::PointAtDistance => &[Parameter::Double],
            Self::StartPoint | Self::EndPoint | Self::IsClosed | Self::CurveToLine => &[],
        }
    }

    fn returns(self, _: Kind) -> Returns {
        match self {
            Self::DistanceToPoint => Returns::Double,
            Self::IsClosed => Returns::Integer,
            Self::StartPoint | Self::EndPoint | Self::PointAtDistance => Returns::Of(Kind::Point),
            Self::CurveToLine => Returns::Of(Kind::LineString),
            Self::PerpPoints => Returns::Of(Kind::Geometry),
        }
    }

    fn warns(self) -> bool {
        false
    }

    fn unit(self) -> Unit {
        match self {
            Self::DistanceToPoint => Unit::Result,
            Self::PointAtDistance => Unit::Argument,
            Self::StartPoint | Self::EndPoint | Self::IsClosed | Self::CurveToLine | Self::PerpPoints => Unit::None,
        }
    }

    fn of(self, g: &Geom, value: &Geometry, rest: &[Value]) -> Result<Warned> {
        if let (Self::PerpPoints, Some(curves)) = (self, multi_curves(value)) {
            return Ok((multi_perp(g, &curves, rest.first())?, None));
        }
        let c = curve(value, self.name())?;
        let geo = &Planar;
        let point = |p: Option<Coord>| p.map_or(Ok(Value::Null), |p| part(g, g.dims, Body::Point(Some(p))));
        let result = match self {
            Self::StartPoint => point(observe::start_point(c, geo)?)?,
            Self::EndPoint => point(observe::end_point(c, geo)?)?,
            Self::IsClosed => truth(observe::is_closed(c, Metric::Plane, geo)?),
            Self::CurveToLine => part(g, g.dims, Body::Curve(Curve::LineString(observe::curve_to_line(c, &Tolerance::DEFAULT, geo)?)))?,
            Self::PointAtDistance => point(observe::point_at_distance(c, number(rest.first(), self.name())?, Metric::Plane, geo)?)?,
            Self::DistanceToPoint => match point_argument(rest.first(), self.name())? {
                Some(p) => double(observe::distance_to_point(c, p, Metric::Plane, geo)?),
                None => Value::Null,
            },
            Self::PerpPoints => match point_argument(rest.first(), self.name())? {
                Some(p) => perpendicular(g, observe::perp_points(c, p, geo)?)?,
                None => Value::Null,
            },
        };
        Ok((result, None))
    }
}

/// The curves of a multicurve or a multilinestring, none of another.
fn multi_curves(value: &Geometry) -> Option<Vec<Curve>> {
    match &value.body {
        Body::Collection(Collection::MultiCurve(curves)) => Some(curves.clone()),
        Body::Collection(Collection::MultiLineString(lines)) => Some(lines.iter().map(|l| Curve::LineString(l.clone())).collect()),
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => None,
    }
}

/// [§10.3.7] ST_PerpPoints(ST_Point) of a multicurve of `curves`: null
/// of the empty set or the empty point, the point if it is on a curve,
/// else the feet on every curve.
fn multi_perp(g: &Geom, curves: &[Curve], arg: Option<&Value>) -> Result<Value> {
    let Some(p) = point_argument(arg, CurveMethod::PerpPoints.name())? else { return Ok(Value::Null) };
    let mut feet = Vec::new();
    for c in curves {
        let Some(found) = observe::perp_points(c, p, &Planar)? else { continue };
        if found == [Foot::Point(p)] {
            return perpendicular(g, Some(found));
        }
        feet.extend(found);
    }
    perpendicular(g, (!curves.is_empty()).then_some(feet))
}

/// [§7.1.15 Description 2e] The feet as a point, a multipoint, a curve, a
/// multicurve or a collection of both, x, y and m; the empty point if
/// none; null of an empty curve.
fn perpendicular(g: &Geom, feet: Option<Vec<Foot>>) -> Result<Value> {
    let Some(feet) = feet else { return Ok(Value::Null) };
    let dims = if g.dims.has_m() { Dims::Xym } else { Dims::Xy };
    let flat = |p: Coord| Coord { z: 0.0, ..p };
    let mut bodies: Vec<Body> = Vec::new();
    for foot in feet {
        let arc = match foot {
            Foot::Point(p) => {
                bodies.push(Body::Point(Some(flat(p))));
                continue;
            }
            Foot::Arc(arc) => arc,
        };
        let [start, middle, end] = arc.map(flat);
        // Arcs meeting end to start are one connected set [Description 2e].
        if let Some(Body::Curve(Curve::CircularString(points))) = bodies.last_mut()
            && points.last() == Some(&start)
        {
            points.extend([middle, end]);
            continue;
        }
        bodies.push(Body::Curve(Curve::CircularString(vec![start, middle, end])));
    }
    let points: Option<Vec<Option<Coord>>> = bodies.iter().map(|b| if let Body::Point(p) = b { Some(*p) } else { None }).collect();
    let arcs: Option<Vec<Curve>> = bodies.iter().map(|b| if let Body::Curve(c) = b { Some(c.clone()) } else { None }).collect();
    let body = match (bodies.len(), points, arcs) {
        (0, _, _) => Body::Point(None),
        (1, _, _) => bodies.into_iter().next().unwrap_or(Body::Point(None)),
        (_, Some(points), _) => Body::Collection(Collection::MultiPoint(points)),
        (_, None, Some(arcs)) => Body::Collection(Collection::MultiCurve(arcs)),
        (_, None, None) => Body::Collection(Collection::Geometry(bodies)),
    };
    part(g, dims, body)
}

/// [§7.2 to §7.5] The methods of the curves given by points, and of
/// their arcs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointListMethod {
    /// [§7.2.3, §7.3.3, §7.4.3, §7.5.3, §8.4.3] ST_Points().
    Points,
    /// [§7.2.4, §7.3.4, §7.5.4] ST_NumPoints().
    NumPoints,
    /// [§7.2.5, §7.3.5, §7.4.4, §7.5.5] ST_PointN(INTEGER).
    PointN,
    /// [§7.3.6] ST_NumSegments().
    NumSegments,
    /// [§7.3.7] ST_SegmentN(INTEGER).
    SegmentN,
    /// [§7.3.8] ST_MidPointRep().
    MidPointRep,
    /// [§7.3.9] ST_Bulge().
    Bulge,
    /// [§7.3.11, §7.4.6] ST_Center().
    Center,
    /// [§7.3.12, §7.4.5] ST_Radius().
    Radius,
}

/// The methods of the curves given by points.
pub const POINT_LIST_METHODS: [PointListMethod; 9] = [
    PointListMethod::Points,
    PointListMethod::NumPoints,
    PointListMethod::PointN,
    PointListMethod::NumSegments,
    PointListMethod::SegmentN,
    PointListMethod::MidPointRep,
    PointListMethod::Bulge,
    PointListMethod::Center,
    PointListMethod::Radius,
];

/// The points of `value`: a curve's given by points, or a triangle's
/// three vertices [§8.4.3].
fn points_of<'v>(value: &'v Geometry, name: &str) -> Result<&'v [Coord]> {
    if let Body::Surface(crate::spatial::formats::tree::Surface::Triangle(ring)) = &value.body {
        return Ok(ring.split_last().map_or(&[], |(_, vertices)| vertices));
    }
    observe::points(curve(value, name)?)
        .ok_or_else(|| spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§7.2", &format!("{name} takes a curve of points")))
}

impl Observer for PointListMethod {
    fn name(self) -> &'static str {
        match self {
            Self::Points => "ST_POINTS",
            Self::NumPoints => "ST_NUMPOINTS",
            Self::PointN => "ST_POINTN",
            Self::NumSegments => "ST_NUMSEGMENTS",
            Self::SegmentN => "ST_SEGMENTN",
            Self::MidPointRep => "ST_MIDPOINTREP",
            Self::Bulge => "ST_BULGE",
            Self::Center => "ST_CENTER",
            Self::Radius => "ST_RADIUS",
        }
    }

    fn parameters(self) -> &'static [Parameter] {
        match self {
            Self::PointN | Self::SegmentN => &[Parameter::Integer],
            Self::Points | Self::NumPoints | Self::NumSegments | Self::MidPointRep | Self::Bulge | Self::Center | Self::Radius => &[],
        }
    }

    fn returns(self, _: Kind) -> Returns {
        match self {
            Self::Points | Self::MidPointRep => Returns::Array(Kind::Point),
            Self::NumPoints | Self::NumSegments => Returns::Integer,
            Self::PointN | Self::Center => Returns::Of(Kind::Point),
            Self::SegmentN => Returns::Of(Kind::CircularString),
            Self::Bulge | Self::Radius => Returns::Double,
        }
    }

    fn warns(self) -> bool {
        matches!(self, Self::PointN | Self::SegmentN)
    }

    fn unit(self) -> Unit {
        if self == Self::Radius { Unit::Result } else { Unit::None }
    }

    fn of(self, g: &Geom, value: &Geometry, rest: &[Value]) -> Result<Warned> {
        let points = points_of(value, self.name())?;
        let point = |p: Coord| part(g, g.dims, Body::Point(Some(p)));
        let some = |n: usize| if points.is_empty() { Value::Null } else { int(n) };
        let result = match self {
            Self::Points => {
                if points.is_empty() {
                    Value::Null
                } else {
                    array(g, g.dims, points.iter().map(|p| Body::Point(Some(*p))))?
                }
            }
            Self::NumPoints => some(points.len()),
            Self::PointN => return found(observe::nth(points, whole(rest.first(), self.name())?), point),
            Self::NumSegments => observe::num_segments(points).map_or(Value::Null, int),
            Self::SegmentN => {
                let at = observe::segment_n(points, whole(rest.first(), self.name())?);
                return found(at, |s| part(g, g.dims, Body::Curve(Curve::CircularString(s))));
            }
            Self::MidPointRep => match observe::mid_point_rep(points) {
                Some(all) => array(g, g.dims, all.into_iter().map(|p| Body::Point(Some(p))))?,
                None => Value::Null,
            },
            Self::Bulge | Self::Center | Self::Radius => self.arc(g, value, points)?,
        };
        Ok((result, None))
    }
}

impl PointListMethod {
    /// The bulge, center or radius of a circle or of a circular string's
    /// one arc; null of a straight one [§7.3.9, §7.3.11, §7.3.12].
    fn arc(self, g: &Geom, value: &Geometry, points: &[Coord]) -> Result<Value> {
        let arc =
            if let Body::Curve(Curve::Circle(_)) = &value.body { <[Coord; 3]>::try_from(points).ok() } else { circular::single(points)? };
        let Some(arc) = arc else { return Ok(Value::Null) };
        if self == Self::Bulge {
            return Ok(Value::Approx(circular::bulge(arc, false).0));
        }
        let Some((center, radius)) = circular::center(arc) else { return Ok(Value::Null) };
        if self == Self::Radius {
            return Ok(Value::Approx(radius));
        }
        part(g, g.dims, Body::Point(Some(Coord { m: 0.0, ..center })))
    }
}

/// [§7.6 to §7.9] The parameters of the parametric curves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterMethod {
    /// [§7.6.8, §7.7.6, §7.8.7, §7.9.8] ST_StartM().
    StartM,
    /// [§7.6.9, §7.7.7, §7.8.8, §7.9.9] ST_EndM().
    EndM,
    /// [§7.6.4] ST_UAxisLength().
    UAxisLength,
    /// [§7.6.5] ST_VAxisLength().
    VAxisLength,
    /// [§7.7.3] ST_Degree().
    Degree,
    /// [§7.8.4] ST_ScaleFactor().
    ScaleFactor,
    /// [§7.8.5] ST_StartDistance().
    StartDistance,
    /// [§7.8.6] ST_EndDistance().
    EndDistance,
    /// [§7.9.5] ST_StartCurvature().
    StartCurvature,
    /// [§7.9.6] ST_EndCurvature().
    EndCurvature,
    /// [§7.9.7] ST_SpiralType().
    SpiralType,
}

/// The parameters of the parametric curves.
pub const PARAMETER_METHODS: [ParameterMethod; 11] = [
    ParameterMethod::StartM,
    ParameterMethod::EndM,
    ParameterMethod::UAxisLength,
    ParameterMethod::VAxisLength,
    ParameterMethod::Degree,
    ParameterMethod::ScaleFactor,
    ParameterMethod::StartDistance,
    ParameterMethod::EndDistance,
    ParameterMethod::StartCurvature,
    ParameterMethod::EndCurvature,
    ParameterMethod::SpiralType,
];

impl Observer for ParameterMethod {
    fn name(self) -> &'static str {
        match self {
            Self::StartM => "ST_STARTM",
            Self::EndM => "ST_ENDM",
            Self::UAxisLength => "ST_UAXISLENGTH",
            Self::VAxisLength => "ST_VAXISLENGTH",
            Self::Degree => "ST_DEGREE",
            Self::ScaleFactor => "ST_SCALEFACTOR",
            Self::StartDistance => "ST_STARTDISTANCE",
            Self::EndDistance => "ST_ENDDISTANCE",
            Self::StartCurvature => "ST_STARTCURVATURE",
            Self::EndCurvature => "ST_ENDCURVATURE",
            Self::SpiralType => "ST_SPIRALTYPE",
        }
    }

    fn parameters(self) -> &'static [Parameter] {
        &[]
    }

    fn returns(self, _: Kind) -> Returns {
        match self {
            Self::Degree => Returns::Integer,
            Self::SpiralType => Returns::Text,
            Self::StartM
            | Self::EndM
            | Self::UAxisLength
            | Self::VAxisLength
            | Self::ScaleFactor
            | Self::StartDistance
            | Self::EndDistance
            | Self::StartCurvature
            | Self::EndCurvature => Returns::Double,
        }
    }

    fn warns(self) -> bool {
        false
    }

    fn unit(self) -> Unit {
        match self {
            Self::UAxisLength | Self::VAxisLength | Self::StartDistance | Self::EndDistance => Unit::Result,
            Self::StartM | Self::EndM | Self::Degree | Self::ScaleFactor | Self::StartCurvature | Self::EndCurvature | Self::SpiralType => {
                Unit::None
            }
        }
    }

    fn of(self, _: &Geom, value: &Geometry, _: &[Value]) -> Result<Warned> {
        let c = curve(value, self.name())?;
        let measures = match c {
            Curve::Elliptical(e) => e.as_ref().map(|e| e.measures),
            Curve::Nurbs(n) => n.as_ref().map(|n| n.measures),
            Curve::Clothoid(k) => k.as_ref().map(|k| k.measures),
            Curve::Spiral(s) => s.as_ref().map(|s| s.measures),
            Curve::LineString(_) | Curve::CircularString(_) | Curve::Circle(_) | Curve::GeodesicString(_) | Curve::Compound(_) => None,
        };
        let result = match (self, c) {
            (Self::StartM, _) => double(measures.flatten().map(|[start, _]| start)),
            (Self::EndM, _) => double(measures.flatten().map(|[_, end]| end)),
            (Self::Degree, Curve::Nurbs(n)) => n.as_ref().map_or(Value::Null, |n| int(usize::try_from(n.degree).unwrap_or(usize::MAX))),
            (Self::UAxisLength, Curve::Elliptical(e)) => double(e.as_ref().map(|e| e.u_axis)),
            (Self::VAxisLength, Curve::Elliptical(e)) => double(e.as_ref().map(|e| e.v_axis)),
            (Self::ScaleFactor, Curve::Clothoid(k)) => double(k.as_ref().map(|k| k.scale)),
            (Self::StartDistance, Curve::Clothoid(k)) => double(k.as_ref().map(|k| k.start)),
            (Self::EndDistance, Curve::Clothoid(k)) => double(k.as_ref().map(|k| k.end)),
            (Self::StartCurvature, Curve::Spiral(s)) => double(s.as_ref().map(|s| s.start_curvature)),
            (Self::EndCurvature, Curve::Spiral(s)) => double(s.as_ref().map(|s| s.end_curvature)),
            (Self::SpiralType, Curve::Spiral(s)) => s.as_ref().map_or(Value::Null, |s| Value::Char(s.kind.name().to_owned())),
            (
                Self::UAxisLength
                | Self::VAxisLength
                | Self::Degree
                | Self::ScaleFactor
                | Self::StartDistance
                | Self::EndDistance
                | Self::StartCurvature
                | Self::EndCurvature
                | Self::SpiralType,
                _,
            ) => {
                return Err(spatial(
                    SqlState::SPATIAL_INVALID_ARGUMENT,
                    "§7.6 to §7.9",
                    &format!("{} takes no curve of this type", self.name()),
                ));
            }
        };
        Ok((result, None))
    }
}

/// [§7.10] The methods of ST_CompoundCurve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompoundMethod {
    /// [§7.10.3] ST_Curves().
    Curves,
    /// [§7.10.4] ST_NumCurves().
    NumCurves,
    /// [§7.10.5] ST_CurveN(INTEGER).
    CurveN,
}

/// The methods of ST_CompoundCurve.
pub const COMPOUND_METHODS: [CompoundMethod; 3] = [CompoundMethod::Curves, CompoundMethod::NumCurves, CompoundMethod::CurveN];

impl Observer for CompoundMethod {
    fn name(self) -> &'static str {
        match self {
            Self::Curves => "ST_CURVES",
            Self::NumCurves => "ST_NUMCURVES",
            Self::CurveN => "ST_CURVEN",
        }
    }

    fn parameters(self) -> &'static [Parameter] {
        if self == Self::CurveN { &[Parameter::Integer] } else { &[] }
    }

    fn returns(self, _: Kind) -> Returns {
        match self {
            Self::Curves => Returns::Array(Kind::Curve),
            Self::NumCurves => Returns::Integer,
            Self::CurveN => Returns::Of(Kind::Curve),
        }
    }

    fn warns(self) -> bool {
        self == Self::CurveN
    }

    fn of(self, g: &Geom, value: &Geometry, rest: &[Value]) -> Result<Warned> {
        let Curve::Compound(parts) = curve(value, self.name())? else {
            return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§7.10", &format!("{} takes a compound curve", self.name())));
        };
        if parts.is_empty() {
            return Ok((Value::Null, None));
        }
        let result = match self {
            Self::Curves => array(g, g.dims, parts.iter().cloned().map(Body::Curve))?,
            Self::NumCurves => int(parts.len()),
            Self::CurveN => return found(observe::nth(parts, whole(rest.first(), self.name())?), |c| part(g, g.dims, Body::Curve(c))),
        };
        Ok((result, None))
    }
}
