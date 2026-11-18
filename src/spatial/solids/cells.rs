//! \[DESIGN\] A geometry value taken apart for the 3D engine: its points,
//! its segments, the triangles of its surfaces and the closed, outward
//! triangle meshes of its solids, all in x, y and z (z zero without one,
//! m dropped). Curves are taken by their linear form (`Geom::linear`);
//! each surface patch is triangulated in its own plane
//! ([`super::triangulate`]); a
//! solid's shells are turned so the exterior faces out and each interior
//! shell faces into its cavity, away from the material. Then
//! [`Cells::exact`] gives the plane-based pieces of one shared [`Scale`].

use super::exact::Scale;
use super::kernel::{Hom, Vector};
use super::pieces::{Poly, Seg};
use super::triangulate::polygon;
use crate::error::Result;
use crate::geometry::model::{Body, Collection, Coords, Curve, Dims, Geom, Rings, Surface};

/// A point in space.
pub type P3 = [f64; 3];

/// The parts of a geometry value in space.
#[derive(Debug, Default, Clone)]
pub struct Cells {
    /// Its points.
    pub points: Vec<P3>,
    /// Its curves' segments.
    pub segments: Vec<[P3; 2]>,
    /// Its surfaces' triangles.
    pub triangles: Vec<[P3; 3]>,
    /// Its solids, each the triangles of its shells, facing out of the
    /// material.
    pub solids: Vec<Vec<[P3; 3]>>,
}

/// The point at `at` of flat `coords` of `dims`, z zero without one.
pub fn point_at(coords: &[f64], dims: Dims, at: usize) -> P3 {
    let stride = dims.stride();
    let base = at.saturating_mul(stride);
    let get = |k: usize| coords.get(base.saturating_add(k)).copied().unwrap_or_default();
    [get(0), get(1), dims.z_at().map_or(0.0_f64, get)]
}

/// The points of flat `coords`.
pub fn points_of(coords: &Coords, dims: Dims) -> Vec<P3> {
    let count = coords.len().checked_div(dims.stride()).unwrap_or_default();
    (0..count).map(|at| point_at(coords, dims, at)).collect()
}

/// The rings of a polygon as points.
pub fn rings_of(rings: &Rings, dims: Dims) -> Vec<Vec<P3>> {
    rings.iter().map(|r| points_of(r, dims)).collect()
}

/// Whether two points are the same, bit for bit.
pub fn same(a: P3, b: P3) -> bool {
    a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

impl Cells {
    /// The parts of `g`, its curves linearized.
    ///
    /// # Errors
    ///
    /// As `Geom::linear`, and as the triangulation of a patch.
    pub fn of(g: &Geom) -> Result<Cells> {
        let mut cells = Cells::default();
        let mut work = vec![g.body.clone()];
        while let Some(body) = work.pop() {
            match body {
                Body::Solid(shells) => {
                    let linear: Vec<Geom> = shells.into_iter().map(|s| g.part(Body::Surface(s)).linear()).collect::<Result<_>>()?;
                    cells.solid(&linear)?;
                }
                Body::Collection(Collection::Geometry(parts)) => work.extend(parts),
                other @ (Body::Point(_)
                | Body::Curve(_)
                | Body::Surface(_)
                | Body::Collection(
                    Collection::MultiPoint(_)
                    | Collection::MultiCurve(_)
                    | Collection::MultiLineString(_)
                    | Collection::MultiSurface(_)
                    | Collection::MultiPolygon(_),
                )) => cells.add(&g.part(other).linear()?)?,
            }
        }
        Ok(cells)
    }

    /// Adds the parts of `linear`, a value of no solid.
    fn add(&mut self, linear: &Geom) -> Result<()> {
        let mut work = vec![&linear.body];
        while let Some(body) = work.pop() {
            match body {
                Body::Point(c) => self.points.extend(points_of(c, linear.dims)),
                Body::Curve(c) => self.curve(c, linear.dims),
                Body::Surface(s) => self.surface(s, linear.dims)?,
                Body::Solid(_) => {}
                Body::Collection(c) => self.collection(c, linear.dims, &mut work)?,
            }
        }
        Ok(())
    }

    fn collection<'g>(&mut self, c: &'g Collection, dims: Dims, work: &mut Vec<&'g Body>) -> Result<()> {
        match c {
            Collection::Geometry(parts) => work.extend(parts),
            Collection::MultiPoint(points) => self.points.extend(points.iter().flat_map(|p| points_of(p, dims))),
            Collection::MultiCurve(curves) => curves.iter().for_each(|part| self.curve(part, dims)),
            Collection::MultiLineString(lines) => lines.iter().for_each(|l| self.line(l, dims)),
            Collection::MultiSurface(surfaces) => surfaces.iter().try_for_each(|s| self.surface(s, dims))?,
            Collection::MultiPolygon(polygons) => polygons.iter().try_for_each(|p| self.patch(&rings_of(p, dims)))?,
        }
        Ok(())
    }

    fn curve(&mut self, c: &Curve, dims: Dims) {
        match c {
            Curve::LineString(p) | Curve::GeodesicString(p) | Curve::CircularString(p) | Curve::Circle(p) => self.line(p, dims),
            Curve::Compound(parts) => parts.iter().for_each(|part| self.curve(part, dims)),
            Curve::Elliptical(_) | Curve::Nurbs(_) | Curve::Clothoid(_) | Curve::Spiral(_) => {}
        }
    }

    fn line(&mut self, coords: &Coords, dims: Dims) {
        let points = points_of(coords, dims);
        let before = self.segments.len();
        self.segments.extend(points.windows(2).filter_map(|w| match w {
            [a, b] if !same(*a, *b) => Some([*a, *b]),
            _ => None,
        }));
        if self.segments.len() == before {
            self.points.extend(points.first());
        }
    }

    fn surface(&mut self, s: &Surface, dims: Dims) -> Result<()> {
        for patch in patches(s, dims) {
            self.patch(&patch)?;
        }
        Ok(())
    }

    /// Triangulates one patch; a patch of no area adds its edges.
    fn patch(&mut self, rings: &[Vec<P3>]) -> Result<()> {
        let triangles = polygon(rings)?;
        if triangles.is_empty() {
            for ring in rings {
                self.segments.extend(ring.windows(2).filter_map(|w| match w {
                    [a, b] if !same(*a, *b) => Some([*a, *b]),
                    _ => None,
                }));
            }
        }
        self.triangles.extend(triangles);
        Ok(())
    }

    /// Adds a solid of linear shells, exterior first.
    fn solid(&mut self, shells: &[Geom]) -> Result<()> {
        let mut faces = Vec::new();
        for (at, shell) in shells.iter().enumerate() {
            let mut part = Cells::default();
            part.add(shell)?;
            let mut triangles = part.triangles;
            let outward = signed_volume(&triangles) >= 0.0_f64;
            // [§9.2.1 D9] The exterior shell bounds the material from
            // outside, an interior shell from inside its cavity.
            if outward != (at == 0) {
                for triangle in &mut triangles {
                    triangle.swap(1, 2);
                }
            }
            faces.extend(triangles);
        }
        self.solids.push(faces);
        Ok(())
    }

    /// The triangles of one patch, its rings as points.
    ///
    /// # Errors
    ///
    /// As the triangulation's.
    pub fn of_patch(rings: &[Vec<P3>]) -> Result<Cells> {
        let mut cells = Cells::default();
        cells.patch(rings)?;
        Ok(cells)
    }

    /// Whether it has no part.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.segments.is_empty() && self.triangles.is_empty() && self.solids.is_empty()
    }

    /// Every coordinate of every part.
    pub fn numbers(&self) -> impl Iterator<Item = f64> + '_ {
        let points = self.points.iter().flatten().copied();
        let segments = self.segments.iter().flatten().flatten().copied();
        let triangles = self.triangles.iter().flatten().flatten().copied();
        let solids = self.solids.iter().flatten().flatten().flatten().copied();
        points.chain(segments).chain(triangles).chain(solids)
    }

    /// The plane-based pieces of its parts at `scale`.
    pub fn exact(&self, scale: Scale) -> Pieces {
        let int = |p: &P3| -> Vector { p.map(|v| scale.integer(v)) };
        let polys = |ts: &[[P3; 3]]| ts.iter().filter_map(|[a, b, c]| Poly::triangle(&int(a), &int(b), &int(c))).collect::<Vec<_>>();
        Pieces {
            points: self.points.iter().map(|p| Hom::of(int(p))).collect(),
            segments: self.segments.iter().filter_map(|[a, b]| Seg::of(&int(a), &int(b))).collect(),
            polys: polys(&self.triangles),
            solids: self.solids.iter().map(|s| polys(s)).filter(|s| !s.is_empty()).collect(),
        }
    }
}

/// The plane-based parts of a geometry value.
#[derive(Debug, Default, Clone)]
pub struct Pieces {
    /// Its points.
    pub points: Vec<Hom>,
    /// Its segments.
    pub segments: Vec<Seg>,
    /// Its surfaces' pieces.
    pub polys: Vec<Poly>,
    /// Its solids' boundary pieces, facing out of the material.
    pub solids: Vec<Vec<Poly>>,
}

impl Pieces {
    /// Whether it has no part.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.segments.is_empty() && self.polys.is_empty() && self.solids.is_empty()
    }
}

/// The patches of surface `s`, each its rings as points: a polygon, a
/// triangle, each patch of a polyhedral surface, each triangle of a TIN,
/// and the patches of each part of a compound surface. A curve polygon
/// has its rings' points, as the linear form gives them.
pub fn patches(s: &Surface, dims: Dims) -> Vec<Vec<Vec<P3>>> {
    let mut out = Vec::new();
    let mut work = vec![s];
    while let Some(next) = work.pop() {
        match next {
            Surface::Polygon(rings) => out.push(rings_of(rings, dims)),
            Surface::Triangle(ring) => {
                if !ring.is_empty() {
                    out.push(vec![points_of(ring, dims)]);
                }
            }
            Surface::PolyhdrlSurface(ps) => out.extend(ps.iter().map(|p| rings_of(p, dims))),
            Surface::Tin(tin) => out.extend(tin.triangles.iter().map(|t| vec![points_of(t, dims)])),
            Surface::Compound(parts) => work.extend(parts.iter().rev()),
            Surface::CurvePolygon(rings) => out.push(rings.iter().filter_map(|r| ring_points(r, dims)).collect()),
        }
    }
    out
}

/// The points of a linear ring curve.
fn ring_points(c: &Curve, dims: Dims) -> Option<Vec<P3>> {
    match c {
        Curve::LineString(p) | Curve::GeodesicString(p) | Curve::CircularString(p) | Curve::Circle(p) => Some(points_of(p, dims)),
        Curve::Compound(parts) => {
            let mut out: Vec<P3> = Vec::new();
            for part in parts {
                let points = ring_points(part, dims)?;
                let skip = usize::from(out.last().zip(points.first()).is_some_and(|(a, b)| same(*a, *b)));
                out.extend(points.into_iter().skip(skip));
            }
            Some(out)
        }
        Curve::Elliptical(_) | Curve::Nurbs(_) | Curve::Clothoid(_) | Curve::Spiral(_) => None,
    }
}

/// The signed volume a closed triangle mesh bounds: positive when it
/// faces out (the divergence theorem over tetrahedra from the origin).
pub fn signed_volume(triangles: &[[P3; 3]]) -> f64 {
    let sixth = 1.0_f64 / 6.0_f64;
    let origin = triangles.first().map_or([0.0_f64; 3], |t| t[0]);
    triangles.iter().map(|[a, b, c]| triple(sub(*a, origin), sub(*b, origin), sub(*c, origin)) * sixth).sum()
}

/// `a - b`.
pub fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The triple product `a . (b x c)`.
pub fn triple(a: P3, b: P3, c: P3) -> f64 {
    a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])
}
