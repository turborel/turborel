//! ST_Transform [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.6]: a value
//! moved to another spatial reference system, a system supplied or one
//! the statement's catalog defines (`SpatialRefSys::from_srid_in`).
//!
//! The transform is implementation-defined [Description 3d]. Each point
//! of the value is transformed (`spatial::srs::transform`), z with it
//! and m left as it is, and so is each point that defines a curve, a
//! circular string's and a circle's and a NURBS curve's control points.
//! A curve given by a placement and directions (an elliptical curve, a
//! clothoid or a spiral) and a rational NURBS curve are not points alone,
//! so a value holding one cannot be transformed [Description 3c].

use super::super::data_type;
use super::super::model::{Body, Collection, Coords, Curve, Geom, Surface};
use super::units::{Systems, fault};
use crate::error::Result;
use crate::functions::{CallContext, ScalarFunction};
use crate::spatial::srs::fault::{Condition, Fault, Outcome};
use crate::spatial::srs::system::SpatialRefSys;
use crate::spatial::srs::transform::Transform as Moved;
use crate::value::{DataType, Value};

/// The method ST_Transform(INTEGER), a function of its subject first.
#[derive(Clone, Copy, Debug)]
pub struct Transform;

const NAME: &str = "ST_TRANSFORM";

impl ScalarFunction for Transform {
    fn name(&self) -> &'static str {
        NAME
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [subject, srid] = args else { return Err(super::signature(NAME)) };
        if !data_type().kind().comparable(subject) || !srid.is_numeric() {
            return Err(super::signature(NAME));
        }
        Ok(data_type())
    }

    fn argument_type(&self, position: usize, _known: &[Option<DataType>]) -> Option<DataType> {
        match position {
            0 => Some(data_type()),
            1 => Some(super::INTEGER),
            _ => None,
        }
    }

    /// It reads the statement's reference systems.
    fn uses_context(&self, _arity: usize) -> bool {
        true
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        transform(args, None)
    }

    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        transform(args, cx.spatial())
    }
}

fn transform(args: &[Value], systems: Systems<'_>) -> Result<Value> {
    let [subject, srid] = args else { return Err(super::signature(NAME)) };
    if subject.is_null() || srid.is_null() {
        return Ok(Value::Null);
    }
    let target = super::super::functions::reference(std::slice::from_ref(srid))?;
    let g = Geom::from_value(subject)?;
    // [Description 3a, 3b] the same system, or the empty set.
    if g.srid == target || g.is_empty() {
        return Ok(g.with_srid(target).to_value());
    }
    // [Description 3c] each failure is a failure to transform.
    let failed = |f: &Fault| fault(&Fault::new(Condition::FailedToTransform, f.message.clone()));
    let from = SpatialRefSys::from_srid_in(g.srid, systems).map_err(|f| failed(&f))?;
    let to = SpatialRefSys::from_srid_in(target, systems).map_err(|f| failed(&f))?;
    let moved = from.transform_to(&to).map_err(|f| failed(&f))?;
    let mut out = g.clone().with_srid(target);
    Points { moved: &moved, stride: g.dims.stride(), z: g.dims.has_z() }.body(&mut out.body).map_err(|f| failed(&f))?;
    Ok(out.to_value())
}

/// Moves every point of a body.
struct Points<'a> {
    moved: &'a Moved,
    stride: usize,
    z: bool,
}

fn not_points(what: &str) -> Fault {
    Fault::new(Condition::FailedToTransform, format!("{what} is not given by points alone"))
}

impl Points<'_> {
    fn coords(&self, c: &mut Coords) -> Outcome<()> {
        self.moved.apply_all(c, self.stride, self.z)
    }

    fn body(&self, body: &mut Body) -> Outcome<()> {
        match body {
            Body::Point(c) => self.coords(c),
            Body::Curve(c) => self.curve(c),
            Body::Surface(s) => self.surface(s),
            Body::Solid(shells) => shells.iter_mut().try_for_each(|s| self.surface(s)),
            Body::Collection(c) => match c {
                Collection::Geometry(parts) => parts.iter_mut().try_for_each(|p| self.body(p)),
                Collection::MultiPoint(points) | Collection::MultiLineString(points) => points.iter_mut().try_for_each(|p| self.coords(p)),
                Collection::MultiCurve(curves) => curves.iter_mut().try_for_each(|one| self.curve(one)),
                Collection::MultiSurface(surfaces) => surfaces.iter_mut().try_for_each(|s| self.surface(s)),
                Collection::MultiPolygon(polygons) => polygons.iter_mut().flatten().try_for_each(|r| self.coords(r)),
            },
        }
    }

    fn curve(&self, c: &mut Curve) -> Outcome<()> {
        match c {
            Curve::LineString(p) | Curve::CircularString(p) | Curve::Circle(p) | Curve::GeodesicString(p) => self.coords(p),
            Curve::Compound(parts) => parts.iter_mut().try_for_each(|p| self.curve(p)),
            Curve::Nurbs(None) | Curve::Elliptical(None) | Curve::Clothoid(None) | Curve::Spiral(None) => Ok(()),
            Curve::Nurbs(Some(n)) => {
                if n.points.iter().any(|p| p.weight.is_some()) {
                    return Err(not_points("a rational NURBS curve"));
                }
                n.points.iter_mut().try_for_each(|p| {
                    let width = p.point.len();
                    self.moved.apply_all(&mut p.point, width, self.z)
                })
            }
            Curve::Elliptical(Some(_)) => Err(not_points("an elliptical curve")),
            Curve::Clothoid(Some(_)) => Err(not_points("a clothoid")),
            Curve::Spiral(Some(_)) => Err(not_points("a spiral curve")),
        }
    }

    fn surface(&self, s: &mut Surface) -> Outcome<()> {
        match s {
            Surface::CurvePolygon(rings) => rings.iter_mut().try_for_each(|r| self.curve(r)),
            Surface::Polygon(rings) => rings.iter_mut().try_for_each(|r| self.coords(r)),
            Surface::Triangle(ring) => self.coords(ring),
            Surface::PolyhdrlSurface(patches) => patches.iter_mut().flatten().try_for_each(|r| self.coords(r)),
            Surface::Tin(tin) => {
                tin.triangles.iter_mut().try_for_each(|t| self.coords(t))?;
                tin.elements.iter_mut().try_for_each(|e| self.body(&mut e.geometry))
            }
            Surface::Compound(parts) => parts.iter_mut().try_for_each(|p| self.surface(p)),
        }
    }
}
