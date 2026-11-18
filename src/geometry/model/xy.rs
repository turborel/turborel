//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.6 rule 3] A value of x
//! and y alone: its z and m left out, as the planar relationships and the
//! routines of two dimensions take it.

use super::{Body, Collection, Coords, Curve, Dims, Geom, Placement, Surface};

pub(super) fn of(g: &Geom) -> Geom {
    let mut out = g.clone();
    if g.dims != Dims::Xy {
        let mut p = Project { from: g.dims };
        p.body(&mut out.body);
        out.dims = Dims::Xy;
    }
    out
}

struct Project {
    from: Dims,
}

impl Project {
    fn coords(&self, c: &mut Coords) {
        *c = c.chunks_exact(self.from.stride()).flat_map(|p| p.iter().take(2).copied()).collect();
    }

    fn planar(&self, c: &mut Vec<f64>) {
        *c = c.chunks_exact(self.from.planar()).flat_map(|p| p.iter().take(2).copied()).collect();
    }

    fn placement(&self, p: &mut Placement) {
        self.planar(&mut p.location);
        self.planar(&mut p.directions);
    }

    fn body(&mut self, body: &mut Body) {
        match body {
            Body::Point(c) => self.coords(c),
            Body::Curve(c) => self.curve(c),
            Body::Surface(s) => self.surface(s),
            Body::Solid(shells) => shells.iter_mut().for_each(|s| self.surface(s)),
            Body::Collection(c) => match c {
                Collection::Geometry(parts) => parts.iter_mut().for_each(|p| self.body(p)),
                Collection::MultiPoint(points) | Collection::MultiLineString(points) => points.iter_mut().for_each(|p| self.coords(p)),
                Collection::MultiCurve(curves) => curves.iter_mut().for_each(|one| self.curve(one)),
                Collection::MultiSurface(surfaces) => surfaces.iter_mut().for_each(|s| self.surface(s)),
                Collection::MultiPolygon(polygons) => polygons.iter_mut().flatten().for_each(|r| self.coords(r)),
            },
        }
    }

    fn curve(&mut self, c: &mut Curve) {
        let (placement, measures) = match c {
            Curve::LineString(p) | Curve::CircularString(p) | Curve::Circle(p) | Curve::GeodesicString(p) => {
                self.coords(p);
                return;
            }
            Curve::Compound(parts) => {
                for p in parts {
                    self.curve(p);
                }
                return;
            }
            Curve::Nurbs(n) => {
                if let Some(n) = n {
                    n.points.iter_mut().for_each(|p| self.planar(&mut p.point));
                    n.measures = None;
                }
                return;
            }
            Curve::Elliptical(Some(e)) => (&mut e.placement, &mut e.measures),
            Curve::Clothoid(Some(c)) => (&mut c.placement, &mut c.measures),
            Curve::Spiral(Some(s)) => (&mut s.placement, &mut s.measures),
            Curve::Elliptical(None) | Curve::Clothoid(None) | Curve::Spiral(None) => return,
        };
        self.placement(placement);
        *measures = None;
    }

    fn surface(&mut self, s: &mut Surface) {
        match s {
            Surface::CurvePolygon(rings) => rings.iter_mut().for_each(|r| self.curve(r)),
            Surface::Polygon(rings) => rings.iter_mut().for_each(|r| self.coords(r)),
            Surface::Triangle(ring) => self.coords(ring),
            Surface::PolyhdrlSurface(patches) => patches.iter_mut().flatten().for_each(|r| self.coords(r)),
            Surface::Tin(tin) => {
                tin.triangles.iter_mut().for_each(|t| self.coords(t));
                tin.elements.iter_mut().for_each(|e| self.body(&mut e.geometry));
            }
            Surface::Compound(parts) => parts.iter_mut().for_each(|p| self.surface(p)),
        }
    }
}
