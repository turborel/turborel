//! \[DESIGN\] A value's envelope in x and y, which an R-tree indexes and
//! the planner prunes by (RULES.md D85): it holds every point of the
//! value, and may hold more. Points and linear parts give their own
//! envelope; a circular arc, the box of its whole circle; an elliptical
//! curve, the box of its whole ellipse; a clothoid or a spiral, the box of
//! the disc its arc length reaches from its placement; a NURBS curve, the
//! box of its control points, weighted and not, as it lies in their convex
//! hull [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.10]; a geodesic
//! string, the box of its points widened to the latitude its great circles
//! reach, x and y read as longitude and latitude in degrees [§4.2.8].

use super::{Body, Collection, Coords, Curve, Dims, Geom, Placement, Surface};
use crate::error::Result;
use crate::geometry::MAX_DEPTH;
use crate::spatial::{Bounds, DIMENSIONS};

/// Degrees of a half turn of longitude.
const HALF_TURN: f64 = 180.0_f64;
/// Degrees of latitude at a pole.
const POLE: f64 = 90.0_f64;

pub(super) fn of(g: &Geom) -> Result<Option<Bounds>> {
    let mut b = Envelope { dims: g.dims, bounds: None };
    b.body(&g.body, 1)?;
    Ok(b.bounds)
}

struct Envelope {
    dims: Dims,
    bounds: Option<Bounds>,
}

impl Envelope {
    fn add(&mut self, x: f64, y: f64) -> Result<()> {
        let next = Bounds::point([x, y])?;
        self.bounds = Some(self.bounds.map_or(next, |b| b.union(next)));
        Ok(())
    }

    /// The box of `centre` widened by `reach` in x and y.
    fn around(&mut self, centre: [f64; DIMENSIONS], reach: [f64; DIMENSIONS]) -> Result<()> {
        let [x, y] = centre;
        let [rx, ry] = reach;
        self.add(x - rx, y - ry)?;
        self.add(x + rx, y + ry)
    }

    fn points(&mut self, c: &Coords) -> Result<()> {
        for p in c.chunks_exact(self.dims.stride()) {
            if let [x, y, ..] = p {
                self.add(*x, *y)?;
            }
        }
        Ok(())
    }

    fn body(&mut self, body: &Body, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let deeper = depth.saturating_add(1);
        match body {
            Body::Point(c) => self.points(c),
            Body::Curve(c) => self.curve(c, deeper),
            Body::Surface(s) => self.surface(s, deeper),
            Body::Solid(shells) => shells.iter().try_for_each(|s| self.surface(s, deeper)),
            Body::Collection(c) => match c {
                Collection::Geometry(parts) => parts.iter().try_for_each(|p| self.body(p, deeper)),
                Collection::MultiPoint(points) | Collection::MultiLineString(points) => points.iter().try_for_each(|p| self.points(p)),
                Collection::MultiCurve(curves) => curves.iter().try_for_each(|one| self.curve(one, deeper)),
                Collection::MultiSurface(surfaces) => surfaces.iter().try_for_each(|s| self.surface(s, deeper)),
                Collection::MultiPolygon(polygons) => polygons.iter().flatten().try_for_each(|r| self.points(r)),
            },
        }
    }

    fn curve(&mut self, c: &Curve, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        match c {
            Curve::LineString(p) => self.points(p),
            Curve::CircularString(p) | Curve::Circle(p) => self.arcs(p),
            Curve::GeodesicString(p) => self.geodesic(p),
            Curve::Elliptical(e) => e.as_ref().map_or(Ok(()), |e| {
                let [d1, d2] = basis(&e.placement, self.dims);
                let reach = [(e.u_axis * d1[0]).hypot(e.v_axis * d2[0]), (e.u_axis * d1[1]).hypot(e.v_axis * d2[1])];
                self.around(location(&e.placement), reach)
            }),
            Curve::Clothoid(c) => c.as_ref().map_or(Ok(()), |c| {
                let length = c.start_distance.abs().max(c.end_distance.abs()) * c.scale_factor.abs().max(1.0_f64);
                self.disc(&c.placement, length)
            }),
            Curve::Spiral(s) => s.as_ref().map_or(Ok(()), |s| self.disc(&s.placement, s.length.abs())),
            Curve::Nurbs(n) => n.as_ref().map_or(Ok(()), |n| {
                for p in &n.points {
                    let (x, y) = (p.point.first().copied().unwrap_or_default(), p.point.get(1).copied().unwrap_or_default());
                    self.add(x, y)?;
                    if let Some(w) = p.weight {
                        self.add(x / w, y / w)?;
                    }
                }
                Ok(())
            }),
            Curve::Compound(parts) => parts.iter().try_for_each(|p| self.curve(p, depth.saturating_add(1))),
        }
    }

    /// The disc of radius `length` scaled by the placement's longest
    /// direction about its location.
    fn disc(&mut self, p: &Placement, length: f64) -> Result<()> {
        let [d1, d2] = basis(p, self.dims);
        let scale = d1[0].hypot(d1[1]).max(d2[0].hypot(d2[1]));
        let r = length * scale;
        self.around(location(p), [r, r])
    }

    /// Arcs of three points each, every next one starting at the last's
    /// end: the box of each one's circle, or of its points if collinear.
    fn arcs(&mut self, p: &Coords) -> Result<()> {
        self.points(p)?;
        let stride = self.dims.stride();
        let xy: Vec<[f64; DIMENSIONS]> =
            p.chunks_exact(stride).map(|c| [c.first().copied().unwrap_or_default(), c.get(1).copied().unwrap_or_default()]).collect();
        let mut at = 0;
        while let (Some(a), Some(b), Some(c)) = (xy.get(at), xy.get(at.saturating_add(1)), xy.get(at.saturating_add(2))) {
            if let Some((centre, r)) = circle(*a, *b, *c) {
                self.around(centre, [r, r])?;
            }
            at = at.saturating_add(2);
        }
        Ok(())
    }

    fn geodesic(&mut self, p: &Coords) -> Result<()> {
        self.points(p)?;
        let stride = self.dims.stride();
        let points: Vec<&[f64]> = p.chunks_exact(stride).collect();
        for pair in points.windows(2) {
            let (Some([x1, y1, ..]), Some([x2, y2, ..])) = (pair.first(), pair.get(1)) else { continue };
            if (x1 - x2).abs() > HALF_TURN {
                self.add(-HALF_TURN, *y1)?;
                self.add(HALF_TURN, *y2)?;
            }
            let reach = vertex_latitude([*x1, *y1], [*x2, *y2]);
            if *y1 >= 0.0_f64 || *y2 >= 0.0_f64 {
                self.add(*x1, reach)?;
            }
            if *y1 <= 0.0_f64 || *y2 <= 0.0_f64 {
                self.add(*x1, -reach)?;
            }
        }
        Ok(())
    }

    fn surface(&mut self, s: &Surface, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        match s {
            Surface::CurvePolygon(rings) => rings.iter().try_for_each(|r| self.curve(r, depth.saturating_add(1))),
            Surface::Polygon(rings) => rings.iter().try_for_each(|r| self.points(r)),
            Surface::Triangle(ring) => self.points(ring),
            Surface::PolyhdrlSurface(patches) => patches.iter().flatten().try_for_each(|r| self.points(r)),
            Surface::Tin(tin) => tin.triangles.iter().try_for_each(|t| self.points(t)),
            Surface::Compound(parts) => parts.iter().try_for_each(|p| self.surface(p, depth.saturating_add(1))),
        }
    }
}

fn location(p: &Placement) -> [f64; DIMENSIONS] {
    [p.location.first().copied().unwrap_or_default(), p.location.get(1).copied().unwrap_or_default()]
}

/// The x and y of a placement's first two reference directions; of one
/// not given, the unit vector of its axis [§17.3].
fn basis(p: &Placement, dims: Dims) -> [[f64; DIMENSIONS]; 2] {
    let planar = dims.planar();
    let direction = |at: usize, unit: [f64; DIMENSIONS]| {
        let start = at.saturating_mul(planar);
        match (p.directions.get(start), p.directions.get(start.saturating_add(1))) {
            (Some(x), Some(y)) => [*x, *y],
            (Some(_) | None, _) => unit,
        }
    };
    [direction(0, [1.0_f64, 0.0_f64]), direction(1, [0.0_f64, 1.0_f64])]
}

/// The centre and radius of the circle through three points, none if they
/// are collinear.
fn circle(start: [f64; DIMENSIONS], mid: [f64; DIMENSIONS], end: [f64; DIMENSIONS]) -> Option<([f64; DIMENSIONS], f64)> {
    let [ax, ay] = start;
    let [bx, by] = [mid[0] - ax, mid[1] - ay];
    let [cx, cy] = [end[0] - ax, end[1] - ay];
    let twice = 2.0_f64 * (bx * cy - by * cx);
    if twice == 0.0_f64 || !twice.is_finite() {
        return None;
    }
    let b2 = bx * bx + by * by;
    let c2 = cx * cx + cy * cy;
    let ux = (cy * b2 - by * c2) / twice;
    let uy = (bx * c2 - cx * b2) / twice;
    // A radius rounded low by the division is widened by a part in 2^40.
    let widened = ux.hypot(uy) * (1.0_f64 + f64::EPSILON * 4096.0_f64);
    (widened.is_finite()).then_some(([ax + ux, ay + uy], widened))
}

/// The highest latitude, in degrees, of the great circle through two
/// points of longitude and latitude in degrees, at most a pole's.
fn vertex_latitude(a: [f64; DIMENSIONS], b: [f64; DIMENSIONS]) -> f64 {
    let unit = |[lon, lat]: [f64; DIMENSIONS]| {
        let (lon, lat) = (lon.to_radians(), lat.to_radians());
        [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()]
    };
    let ([ax, ay, az], [bx, by, bz]) = (unit(a), unit(b));
    let normal = [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx];
    let length = normal[0].hypot(normal[1]).hypot(normal[2]);
    if length == 0.0_f64 || !length.is_finite() {
        return POLE;
    }
    let tilt = (normal[2] / length).abs().min(1.0_f64).acos().to_degrees();
    // The vertex is the pole of the normal, rounded up by a hundredth of a
    // degree so a value rounded low still holds the arc.
    (tilt + 0.01_f64).min(POLE).max(a[1].abs()).max(b[1].abs())
}
