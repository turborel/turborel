//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §6 to §10, §17] What every
//! geometry value is, checked as one is made and as one is read: finite
//! ordinates, the counts and closure of each part, curves of a compound
//! curve end to end, and the representation's limits (RULES.md
//! D119). Simplicity, validity and planarity are routines'
//! [§5.1.8, §5.1.10], not the model's.

use super::{Body, Collection, Coords, Curve, Dims, Geom, Measures, Nurbs, Placement, Surface, Tin, TinElement};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::geometry::{MAX_DEPTH, MAX_PARTS};

/// [§7.3] An arc is three points.
const ARC: usize = 3;
/// [§8.3, §8.4] A ring has at least four points; a triangle's exactly.
pub(super) const MIN_RING: usize = 4;
/// [§4.9.3] A placement has at most three reference directions.
const MAX_DIRECTIONS: usize = 3;
/// [§5.1.68 Description co] An element type is at most 30 characters.
pub(super) const ELEMENT_TYPE_LENGTH: usize = 30;
/// [§5.1.68 Description cq] An element tag is at most 64 characters.
pub(super) const ELEMENT_TAG_LENGTH: usize = 64;

/// The error of a value that is no value of its type: SQLSTATE 2FF02,
/// invalid argument [§22], the rule `rule` broken.
pub(crate) fn invalid(rule: &str, detail: &str) -> SqlError {
    failed(SqlState::SPATIAL_INVALID_ARGUMENT, rule, detail)
}

/// The spatial condition `state` of the rule `rule`.
pub(crate) fn failed(state: SqlState, rule: &str, detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {rule}] {detail}")).with_state(state)
}

/// A representation limit passed (RULES.md D85), SQLCODE -900.
pub(crate) fn limited(count: usize, maximum: usize) -> Result<()> {
    if count > maximum {
        return Err(SqlError::new(
            Code::Unsupported,
            "[DESIGN] a geometry passes this representation's size or nesting limit (RULES.md D85)",
        ));
    }
    Ok(())
}

/// Every ordinate and parameter of `g` as positive zero if it is negative
/// zero, so values equal as numbers have one representation.
pub(super) fn normalize(g: &mut Geom) {
    super::parts::numbers_mut(&mut g.body, &mut |n: &mut f64| *n += 0.0_f64);
}

/// That `g` is a value of its type, within the limits.
pub(super) fn check(g: &Geom) -> Result<()> {
    let mut walk = Walk { dims: g.dims, parts: 0 };
    walk.body(&g.body, 1)?;
    if matches!(g.body, Body::Solid(_)) && g.dims != Dims::Xyz {
        return Err(failed(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, "§5.1.68", "an ST_BRepSolid has z and no m"));
    }
    Ok(())
}

/// A walk over a value, counting its parts against `MAX_PARTS`.
struct Walk {
    dims: Dims,
    parts: usize,
}

impl Walk {
    fn charge(&mut self, count: usize) -> Result<()> {
        self.parts = self.parts.saturating_add(count);
        limited(self.parts, MAX_PARTS)
    }

    fn body(&mut self, body: &Body, depth: usize) -> Result<()> {
        limited(depth, MAX_DEPTH)?;
        self.charge(1)?;
        match body {
            Body::Point(c) => self.point(c),
            Body::Curve(c) => self.curve(c, depth),
            Body::Surface(s) => self.surface(s, depth),
            Body::Solid(shells) => self.solid(shells, depth),
            Body::Collection(c) => self.collection(c, depth),
        }
    }

    fn point(&mut self, c: &Coords) -> Result<()> {
        if !c.is_empty() && c.len() != self.dims.stride() {
            return Err(invalid("§6.1", "a point has one ordinate of each coordinate dimension"));
        }
        self.coords(c).map(drop)
    }

    /// That `c` is whole points of finite ordinates, and charges them.
    fn coords(&mut self, c: &Coords) -> Result<usize> {
        let stride = self.dims.stride();
        if !c.len().is_multiple_of(stride) {
            return Err(invalid("§5.1.68", "a point has one ordinate of each coordinate dimension"));
        }
        finite(c)?;
        let count = c.len().checked_div(stride).unwrap_or_default();
        self.charge(count)?;
        Ok(count)
    }

    /// A linestring's or a geodesic string's points: none or at least two.
    fn line(&mut self, p: &Coords) -> Result<()> {
        if self.coords(p)? == 1 {
            return Err(invalid("§7.2", "a curve of points that is not empty has at least two"));
        }
        Ok(())
    }

    fn curve(&mut self, c: &Curve, depth: usize) -> Result<()> {
        match c {
            Curve::LineString(p) | Curve::GeodesicString(p) => self.line(p)?,
            Curve::CircularString(p) => {
                let n = self.coords(p)?;
                if n != 0 && (n < ARC || n % 2 == 0) {
                    return Err(invalid("§4.2.6", "a circular string has an odd number of points, at least three"));
                }
            }
            Curve::Circle(p) => {
                if !matches!(self.coords(p)?, 0 | ARC) {
                    return Err(failed(SqlState::SPATIAL_EXACTLY_THREE_POINTS, "§4.2.7", "a circle has three control points"));
                }
            }
            Curve::Elliptical(e) => {
                self.parametric(e.as_ref().map(|e| (&e.placement, e.measures, [e.u_axis, e.v_axis, e.start_angle, e.end_angle])))?;
            }
            Curve::Clothoid(c) => {
                let numbers = |k: &super::Clothoid| [k.scale_factor, k.start_distance, k.end_distance, k.end_distance];
                self.parametric(c.as_ref().map(|c| (&c.placement, c.measures, numbers(c))))?;
            }
            Curve::Spiral(s) => {
                self.parametric(s.as_ref().map(|s| (&s.placement, s.measures, [s.length, s.start_curvature, s.end_curvature, s.length])))?;
            }
            Curve::Nurbs(n) => self.nurbs(n.as_ref())?,
            Curve::Compound(parts) => self.compound(parts, depth)?,
        }
        Ok(())
    }

    /// A curve given by a placement and parameters.
    fn parametric(&mut self, parts: Option<(&Placement, Option<Measures>, [f64; 4])>) -> Result<()> {
        let Some((placement, measures, numbers)) = parts else { return Ok(()) };
        self.placement(placement)?;
        self.measures(measures)?;
        finite(&numbers)
    }

    fn placement(&mut self, p: &Placement) -> Result<()> {
        let planar = self.dims.planar();
        let directions = p.directions.len().checked_div(planar).unwrap_or_default();
        if p.location.len() != planar || !p.directions.len().is_multiple_of(planar) || directions > MAX_DIRECTIONS {
            return Err(invalid("§17.3", "an affine placement's location and directions have the coordinate dimension of its curve"));
        }
        finite(&p.location)?;
        finite(&p.directions)?;
        self.charge(directions.saturating_add(1))
    }

    fn measures(&self, measures: Option<Measures>) -> Result<()> {
        if measures.is_some() != self.dims.has_m() {
            return Err(failed(
                SqlState::SPATIAL_MISSING_MEASURE_VALUES,
                "§5.1.68",
                "a curve given by parameters has start and end measures exactly when it has m",
            ));
        }
        measures.map_or(Ok(()), |m| finite(&[m.start, m.end]))
    }

    fn nurbs(&mut self, n: Option<&Nurbs>) -> Result<()> {
        let Some(n) = n else { return Ok(()) };
        self.measures(n.measures)?;
        super::parts::nurbs(n, self.dims.planar())?;
        self.charge(n.points.len().saturating_add(n.knots.len()))
    }

    fn compound(&mut self, parts: &[Curve], depth: usize) -> Result<()> {
        for part in parts {
            limited(depth.saturating_add(1), MAX_DEPTH)?;
            self.charge(1)?;
            if part.is_empty() {
                return Err(failed(SqlState::SPATIAL_ELEMENT_IS_AN_EMPTY_SET, "§7.10", "a compound curve's curves are not empty"));
            }
            self.curve(part, depth.saturating_add(1))?;
        }
        super::parts::contiguous(parts, self.dims)
    }

    fn surface(&mut self, s: &Surface, depth: usize) -> Result<()> {
        match s {
            Surface::CurvePolygon(rings) => {
                for ring in rings {
                    self.curve(ring, depth.saturating_add(1))?;
                    super::parts::closed_ring(ring, self.dims)?;
                }
                Ok(())
            }
            Surface::Polygon(rings) => self.rings(rings),
            Surface::Triangle(ring) => self.triangle(ring),
            Surface::PolyhdrlSurface(patches) => {
                for patch in patches {
                    self.patch(patch)?;
                }
                Ok(())
            }
            Surface::Tin(tin) => self.tin(tin, depth),
            Surface::Compound(parts) => self.surfaces(parts, depth),
        }
    }

    fn rings(&mut self, rings: &[Coords]) -> Result<()> {
        self.charge(rings.len())?;
        for ring in rings {
            self.coords(ring)?;
            super::parts::ring(ring, self.dims)?;
        }
        Ok(())
    }

    fn patch(&mut self, patch: &[Coords]) -> Result<()> {
        if patch.is_empty() {
            return Err(failed(SqlState::SPATIAL_ELEMENT_IS_AN_EMPTY_SET, "§8.5", "a polyhedral surface's patches are not empty"));
        }
        self.rings(patch)
    }

    fn triangle(&mut self, ring: &Coords) -> Result<()> {
        let n = self.coords(ring)?;
        if n == 0 {
            return Ok(());
        }
        if n != MIN_RING {
            return Err(failed(SqlState::SPATIAL_EXTERIOR_RING_NOT_FOUR_POINTS, "§8.4", "a triangle's ring has four points"));
        }
        super::parts::ring(ring, self.dims)
    }

    fn tin(&mut self, tin: &Tin, depth: usize) -> Result<()> {
        for triangle in &tin.triangles {
            if triangle.is_empty() {
                return Err(failed(SqlState::SPATIAL_ELEMENT_IS_AN_EMPTY_SET, "§8.6", "a TIN's triangles are not empty"));
            }
            self.triangle(triangle)?;
        }
        for element in &tin.elements {
            self.element(element, depth)?;
        }
        if let Some(m) = tin.max_side_length
            && (!m.is_finite() || m <= 0.0_f64)
        {
            return Err(invalid("§8.6", "a TIN's maximum side length is finite and positive"));
        }
        Ok(())
    }

    fn element(&mut self, e: &TinElement, depth: usize) -> Result<()> {
        let lengths = !e.element_type.is_empty() && e.element_type.len() <= ELEMENT_TYPE_LENGTH && e.tag.len() <= ELEMENT_TAG_LENGTH;
        if !lengths || !super::parts::letters(&e.element_type) || !super::parts::letters(&e.tag) {
            return Err(invalid("§17.1", "a TIN element's type is 1 to 30 letters and its tag at most 64"));
        }
        self.body(&e.geometry, depth.saturating_add(1))
    }

    fn surfaces(&mut self, parts: &[Surface], depth: usize) -> Result<()> {
        for part in parts {
            limited(depth.saturating_add(1), MAX_DEPTH)?;
            self.charge(1)?;
            if part.is_empty() {
                return Err(failed(SqlState::SPATIAL_ELEMENT_IS_AN_EMPTY_SET, "§8.7", "a compound surface's surfaces are not empty"));
            }
            self.surface(part, depth.saturating_add(1))?;
        }
        Ok(())
    }

    fn solid(&mut self, shells: &[Surface], depth: usize) -> Result<()> {
        for shell in shells {
            if !matches!(shell, Surface::PolyhdrlSurface(_) | Surface::Tin(_) | Surface::Compound(_)) {
                return Err(failed(
                    SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE,
                    "§9.2",
                    "a solid's shells are polyhedral or compound surfaces",
                ));
            }
            if shell.is_empty() {
                return Err(failed(SqlState::SPATIAL_NULL_EXTERIOR_SHELL, "§9.2", "a solid's shells are not empty"));
            }
            self.surface(shell, depth.saturating_add(1))?;
        }
        Ok(())
    }

    fn collection(&mut self, c: &Collection, depth: usize) -> Result<()> {
        let deeper = depth.saturating_add(1);
        limited(deeper, MAX_DEPTH)?;
        match c {
            Collection::Geometry(parts) => parts.iter().try_for_each(|p| self.body(p, deeper)),
            Collection::MultiPoint(points) => points.iter().try_for_each(|p| self.charge(1).and_then(|()| self.point(p))),
            Collection::MultiCurve(curves) => curves.iter().try_for_each(|curve| self.charge(1).and_then(|()| self.curve(curve, deeper))),
            Collection::MultiLineString(lines) => lines.iter().try_for_each(|l| self.charge(1).and_then(|()| self.line(l))),
            Collection::MultiSurface(surfaces) => surfaces.iter().try_for_each(|s| self.charge(1).and_then(|()| self.surface(s, deeper))),
            Collection::MultiPolygon(polygons) => polygons.iter().try_for_each(|p| self.charge(1).and_then(|()| self.rings(p))),
        }
    }
}

/// That every number of `numbers` is finite.
pub(super) fn finite(numbers: &[f64]) -> Result<()> {
    if numbers.iter().all(|n| n.is_finite()) {
        return Ok(());
    }
    Err(failed(SqlState::SPATIAL_POINT_VALUE_NOT_WELL_FORMED, "§5.1.68 Description gd", "an ordinate or a parameter is not finite"))
}
