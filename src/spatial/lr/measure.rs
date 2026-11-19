//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.4.4, §15.4.5, §15.9.1
//! Description 16 to 18] The plane geometry linear referencing measures
//! on: a curve's linear form as a path of vertices, the station at a
//! distance along it, the foot of a point on it, and the station of an m
//! coordinate.
//!
//! Implementation-defined choices, as §15.9.1 leaves how a distance is
//! measured to the LRM: a distance along a curve is measured in the x-y
//! plane, along its linear form (`Geom::linear`), in the units of its
//! coordinates; z and m are interpolated along each segment.

use super::{deeper, failed};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Collection, Coords, Curve, Dims, Geom, Rings, Surface};

/// \[DESIGN\] Implementation-defined: two places are the same, and a point
/// is "precisely on" a curve [§15.4.5 Description 4], when they are
/// within this part of the larger of 1 and the curve's length, the
/// rounding a few operations on binary64 values make.
pub const RELATIVE_TOLERANCE: f64 = 1.0e-9_f64;

/// A vertex: x and y, and z and m where the value has them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Vertex {
    /// x.
    pub x: f64,
    /// y.
    pub y: f64,
    /// z, of a value with z.
    pub z: Option<f64>,
    /// m, of a value with m.
    pub m: Option<f64>,
}

/// A place on a path: its distance along it, its coordinates, and the
/// unit direction of the path there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Station {
    /// The distance along the path from its start.
    pub s: f64,
    /// The place.
    pub at: Vertex,
    /// The x of the unit tangent.
    pub dx: f64,
    /// The y of the unit tangent.
    pub dy: f64,
}

/// The place on a path nearest a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Foot {
    /// The station.
    pub station: Station,
    /// The distance in the plane from the point to it.
    pub distance: f64,
    /// Positive if the point is left of the path, facing along it;
    /// negative if right; zero if on it.
    pub side: f64,
}

/// A curve's linear form: its vertices, and the distance along it of
/// each.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Path {
    /// The vertices, at least one.
    vertices: Vec<Vertex>,
    /// The distance along the path of each vertex.
    along: Vec<f64>,
}

impl Path {
    /// The path of a curve: its linear form, a linestring.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF02 of an empty curve or one whose linear form is no
    /// linestring; as `Geom::linear`.
    pub fn of_curve(g: &Geom) -> Result<Path> {
        let linear = g.linear()?;
        let Body::Curve(c) = &linear.body else { return Err(no_location("the curve's linear form is not a linestring")) };
        let coords = line_coords(c).ok_or_else(|| no_location("the curve's linear form is not a linestring"))?;
        Path::of_coords(coords, linear.dims).ok_or_else(|| no_location("the curve is empty"))
    }

    /// The path through `coords`, points of `dims`; none if there are none.
    pub fn of_coords(coords: &[f64], dims: Dims) -> Option<Path> {
        let vertices: Vec<Vertex> = coords.chunks_exact(dims.stride()).filter_map(|p| vertex(p, dims)).collect();
        let first = vertices.first()?;
        let mut along = Vec::with_capacity(vertices.len());
        let mut total = 0.0_f64;
        let mut previous = *first;
        for v in &vertices {
            total += planar(&previous, v);
            along.push(total);
            previous = *v;
        }
        Some(Path { vertices, along })
    }

    /// Its length in the plane.
    pub fn length(&self) -> f64 {
        self.along.last().copied().unwrap_or_default()
    }

    /// Whether every vertex has an m coordinate.
    pub fn is_measured(&self) -> bool {
        self.vertices.iter().all(|v| v.m.is_some())
    }

    /// The segments of positive length, each with the distance along the
    /// path of its start and its length.
    fn segments(&self) -> impl Iterator<Item = (&Vertex, &Vertex, f64, f64)> + '_ {
        self.vertices.windows(2).zip(&self.along).filter_map(|(pair, &start)| {
            let [a, b] = pair else { return None };
            let len = planar(a, b);
            (len > 0.0_f64).then_some((a, b, start, len))
        })
    }

    /// The station at distance `s` along it, `s` held to its extent.
    pub fn at(&self, s: f64) -> Station {
        let mut chosen = None;
        for (a, b, start, len) in self.segments() {
            chosen = Some((a, b, start, len));
            if s <= start + len {
                break;
            }
        }
        if let Some((a, b, start, len)) = chosen {
            return between(a, b, start, len, ((s - start) / len).clamp(0.0_f64, 1.0_f64));
        }
        let at = self.vertices.first().copied().unwrap_or(Vertex { x: 0.0_f64, y: 0.0_f64, z: None, m: None });
        Station { s: 0.0_f64, at, dx: 1.0_f64, dy: 0.0_f64 }
    }

    /// [§15.4.5 Description 5] The station nearest `(x, y)`; of several
    /// equally near, the one nearest the start.
    pub fn foot(&self, x: f64, y: f64) -> Foot {
        let mut best: Option<Foot> = None;
        for (a, b, start, len) in self.segments() {
            let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
            let t = (((x - a.x) * ux + (y - a.y) * uy) / len).clamp(0.0_f64, 1.0_f64);
            let station = between(a, b, start, len, t);
            let distance = (x - station.at.x).hypot(y - station.at.y);
            if best.is_none_or(|f| distance < f.distance) {
                best = Some(Foot { station, distance, side: ux * (y - a.y) - uy * (x - a.x) });
            }
        }
        best.unwrap_or_else(|| {
            let station = self.at(0.0_f64);
            Foot { station, distance: (x - station.at.x).hypot(y - station.at.y), side: 0.0_f64 }
        })
    }

    /// [§15.9.1 Description 18] The distance along it of the first place
    /// whose m, interpolated between two vertices bracketing it, is `m`.
    pub fn s_at_m(&self, m: f64) -> Option<f64> {
        self.segments().find_map(|(a, b, start, len)| {
            let (ma, mb) = (a.m?, b.m?);
            if m < ma.min(mb) || m > ma.max(mb) {
                return None;
            }
            let span = mb - ma;
            if span.total_cmp(&0.0_f64).is_eq() {
                return Some(start);
            }
            Some(start + (m - ma) / span * len)
        })
    }

    /// The tolerance of places on it: `RELATIVE_TOLERANCE` of the larger
    /// of 1 and its length.
    pub fn tolerance(&self) -> f64 {
        RELATIVE_TOLERANCE * self.length().max(1.0_f64)
    }
}

/// The station `t` of the way from `a` to `b`, a segment beginning `start`
/// along the path, of length `len`.
fn between(a: &Vertex, b: &Vertex, start: f64, len: f64, t: f64) -> Station {
    let at = Vertex { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, z: lerp(a.z, b.z, t), m: lerp(a.m, b.m, t) };
    Station { s: start + len * t, at, dx: (b.x - a.x) / len, dy: (b.y - a.y) / len }
}

/// `t` of the way from `a` to `b`, if both are given.
fn lerp(a: Option<f64>, b: Option<f64>, t: f64) -> Option<f64> {
    Some(a? + (b? - a?) * t)
}

/// The distance in the plane from `a` to `b`.
fn planar(a: &Vertex, b: &Vertex) -> f64 {
    (b.x - a.x).hypot(b.y - a.y)
}

/// The vertex of ordinates `p` of `dims`.
fn vertex(p: &[f64], dims: Dims) -> Option<Vertex> {
    let (&x, rest) = p.split_first()?;
    let &y = rest.first()?;
    Some(Vertex { x, y, z: dims.z_at().and_then(|i| p.get(i).copied()), m: dims.m_at().and_then(|i| p.get(i).copied()) })
}

/// The points of a linestring; none of a curve of another interpolation.
pub(crate) fn line_coords(c: &Curve) -> Option<&Coords> {
    match c {
        Curve::LineString(coords) => Some(coords),
        Curve::CircularString(_)
        | Curve::Circle(_)
        | Curve::GeodesicString(_)
        | Curve::Elliptical(_)
        | Curve::Nurbs(_)
        | Curve::Clothoid(_)
        | Curve::Spiral(_)
        | Curve::Compound(_) => None,
    }
}

/// The rings of a polygon; none of a surface of another kind.
fn polygon_rings(s: &Surface) -> Option<&Rings> {
    match s {
        Surface::Polygon(rings) => Some(rings),
        Surface::CurvePolygon(_) | Surface::Triangle(_) | Surface::PolyhdrlSurface(_) | Surface::Tin(_) | Surface::Compound(_) => None,
    }
}

/// The paths of a geometry's linear form: each point, linestring and ring.
///
/// # Errors
///
/// As `Geom::linear`; SQLCODE -900 past `MAX_DEPTH`.
pub(crate) fn paths_of(g: &Geom) -> Result<Vec<Path>> {
    let linear = g.linear()?;
    let mut sequences = Vec::new();
    sequences_of(&linear.body, &mut sequences, 0)?;
    Ok(sequences.into_iter().filter_map(|c| Path::of_coords(c, linear.dims)).collect())
}

/// Adds the coordinate sequences of `b`, a linear form, to `out`.
fn sequences_of<'g>(b: &'g Body, out: &mut Vec<&'g [f64]>, depth: usize) -> Result<()> {
    let next = deeper(depth)?;
    match b {
        Body::Point(c) => out.push(c),
        Body::Curve(c) => out.extend(line_coords(c).map(Vec::as_slice)),
        Body::Surface(s) => out.extend(polygon_rings(s).into_iter().flatten().map(Vec::as_slice)),
        Body::Solid(_) => {}
        Body::Collection(c) => match c {
            Collection::Geometry(parts) => {
                for p in parts {
                    sequences_of(p, out, next)?;
                }
            }
            Collection::MultiPoint(lines) | Collection::MultiLineString(lines) => out.extend(lines.iter().map(Vec::as_slice)),
            Collection::MultiPolygon(polygons) => out.extend(polygons.iter().flatten().map(Vec::as_slice)),
            Collection::MultiCurve(curves) => out.extend(curves.iter().filter_map(line_coords).map(Vec::as_slice)),
            Collection::MultiSurface(surfaces) => out.extend(surfaces.iter().filter_map(polygon_rings).flatten().map(Vec::as_slice)),
        },
    }
    Ok(())
}

/// The vertex nearest `(x, y)` on any of `paths`: its foot's place.
pub(crate) fn nearest(paths: &[Path], x: f64, y: f64) -> Option<Vertex> {
    let mut best: Option<Foot> = None;
    for p in paths {
        let foot = p.foot(x, y);
        if best.is_none_or(|b| foot.distance < b.distance) {
            best = Some(foot);
        }
    }
    best.map(|f| f.station.at)
}

/// The place of a point value: its vertex.
///
/// # Errors
///
/// SQLSTATE 2FF17 (empty point value) of an empty point; 2FF02 of a
/// geometry that is no point.
pub(crate) fn point_vertex(g: &Geom) -> Result<Vertex> {
    let Body::Point(coords) = &g.body else {
        return Err(failed(
            SqlState::SPATIAL_INVALID_ARGUMENT,
            "§15.4.5 Description 1a",
            &format!("{} is not an ST_Point", g.kind().sql_name()),
        ));
    };
    vertex(coords, g.dims).ok_or_else(|| failed(SqlState::SPATIAL_EMPTY_POINT_VALUE, "§15.4.5 Description 2b", "empty point value"))
}

/// The point value at `v`, of SRID `srid`: with z and m where `v` has
/// them.
///
/// # Errors
///
/// As `Geom::new`, of a coordinate that is not finite.
pub(crate) fn point_geom(v: Vertex, srid: i32) -> Result<Geom> {
    let dims = match (v.z.is_some(), v.m.is_some()) {
        (false, false) => Dims::Xy,
        (true, false) => Dims::Xyz,
        (false, true) => Dims::Xym,
        (true, true) => Dims::Xyzm,
    };
    let coords: Coords = [Some(v.x), Some(v.y), v.z, v.m].into_iter().flatten().collect();
    Geom::new(srid, dims, Body::Point(coords))
}

/// SQLSTATE 2FF02: a curve that has no location, as `detail` says.
fn no_location(detail: &str) -> crate::error::SqlError {
    failed(SqlState::SPATIAL_INVALID_ARGUMENT, "§15.4.1 Description 2", detail)
}
