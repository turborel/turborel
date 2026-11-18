//! ST_LocateAlong and ST_LocateBetween, ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §5.1.13 and §5.1.15: the parts of a measured value whose m lies
//! in a range, z left out. Curves are interpolated linearly in m along
//! their linearization; the operation on a surface or a solid, which
//! the standard leaves to the implementation, is that on its boundary
//! rings.

use super::fault::{Fault, fault};
use super::geodesy::Geodesy;
use super::linearize::Tolerance;
use super::observe::curve_to_line;
use super::types::{Coord, Curve, Dims};
use crate::error::Result;
use crate::spatial::formats::tree::{Body, Collection, Geometry, Surface};

/// The points and the curve runs a value's measures fall on.
#[derive(Default)]
struct Found {
    points: Vec<Coord>,
    lines: Vec<Vec<Coord>>,
}

/// ST_LocateBetween(start, end) of `value`, not empty: the multipoint,
/// the multicurve of line strings or the collection of both whose m lies
/// in the range, an empty point when none does or the value has no m.
///
/// # Errors
///
/// SQLSTATE 2FF02 when `end` is less than `start` or either is not a
/// number; as the linearization's.
pub fn locate_between<G: Geodesy>(value: &Geometry, (start, end): (f64, f64), geo: &G) -> Result<Geometry> {
    if start.is_nan() || end.is_nan() || end < start {
        return Err(fault(Fault::InvalidArgument, "§5.1.15", "the end measure is less than the start measure"));
    }
    if !value.dims.has_m() {
        return Ok(Geometry { dims: Dims::Xy, body: Body::Point(None) });
    }
    let mut found = Found::default();
    found.body(&value.body, (start, end), geo)?;
    Ok(Geometry { dims: Dims::Xym, body: found.result() })
}

impl Found {
    fn body<G: Geodesy>(&mut self, body: &Body, range: (f64, f64), geo: &G) -> Result<()> {
        match body {
            Body::Point(p) => self.point(p.as_ref(), range),
            Body::Curve(c) => self.curve(c, range, geo)?,
            Body::Surface(s) => self.surface(s, range, geo)?,
            Body::Solid(shells) => {
                for s in shells {
                    self.surface(s, range, geo)?;
                }
            }
            Body::Collection(c) => self.collection(c, range, geo)?,
        }
        Ok(())
    }

    fn point(&mut self, p: Option<&Coord>, (start, end): (f64, f64)) {
        if let Some(found) = p.filter(|p| start <= p.m && p.m <= end) {
            self.points.push(flat(*found));
        }
    }

    fn collection<G: Geodesy>(&mut self, c: &Collection, range: (f64, f64), geo: &G) -> Result<()> {
        match c {
            Collection::Geometry(bodies) => bodies.iter().try_for_each(|b| self.body(b, range, geo)),
            Collection::MultiPoint(points) => {
                for p in points {
                    self.point(p.as_ref(), range);
                }
                Ok(())
            }
            Collection::MultiCurve(curves) => curves.iter().try_for_each(|member| self.curve(member, range, geo)),
            Collection::MultiLineString(lines) => {
                for l in lines {
                    self.line(l, range);
                }
                Ok(())
            }
            Collection::MultiSurface(surfaces) => surfaces.iter().try_for_each(|s| self.surface(s, range, geo)),
            Collection::MultiPolygon(polygons) => {
                for l in polygons.iter().flatten() {
                    self.line(l, range);
                }
                Ok(())
            }
        }
    }

    fn surface<G: Geodesy>(&mut self, s: &Surface, range: (f64, f64), geo: &G) -> Result<()> {
        match s {
            Surface::CurvePolygon(rings) => rings.iter().try_for_each(|c| self.curve(c, range, geo)),
            Surface::Polygon(rings) => {
                for l in rings {
                    self.line(l, range);
                }
                Ok(())
            }
            Surface::Triangle(ring) => {
                self.line(ring, range);
                Ok(())
            }
            Surface::Polyhedral(patches) => {
                for l in patches.iter().flatten() {
                    self.line(l, range);
                }
                Ok(())
            }
            Surface::Tin(tin) => {
                for l in &tin.triangles {
                    self.line(l, range);
                }
                Ok(())
            }
            Surface::Compound(parts) => parts.iter().try_for_each(|p| self.surface(p, range, geo)),
        }
    }

    fn curve<G: Geodesy>(&mut self, c: &Curve, range: (f64, f64), geo: &G) -> Result<()> {
        if let Curve::LineString(points) = c {
            self.line(points, range);
            return Ok(());
        }
        let line = curve_to_line(c, &Tolerance::DEFAULT, geo)?;
        self.line(&line, range);
        Ok(())
    }

    /// The runs and the points of the line `points` in the range.
    fn line(&mut self, points: &[Coord], range: (f64, f64)) {
        if let [only] = points {
            self.point(Some(only), range);
        }
        let mut run: Vec<Coord> = Vec::new();
        for pair in points.windows(2) {
            let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else { continue };
            let Some((p, q)) = clip(*a, *b, range) else {
                self.close(&mut run);
                continue;
            };
            if run.last() == Some(&p) {
                if p != q {
                    run.push(q);
                }
                continue;
            }
            self.close(&mut run);
            if p == q {
                // A vertex both its segments reach is one point.
                if self.points.last() != Some(&p) {
                    self.points.push(p);
                }
                continue;
            }
            if self.points.last() == Some(&p) {
                self.points.pop();
            }
            run.extend([p, q]);
        }
        self.close(&mut run);
    }

    fn close(&mut self, run: &mut Vec<Coord>) {
        if run.len() > 1 {
            self.lines.push(std::mem::take(run));
        }
        run.clear();
    }

    fn result(self) -> Body {
        let points: Vec<Option<Coord>> = self.points.into_iter().map(Some).collect();
        match (points.is_empty(), self.lines.is_empty()) {
            (true, true) => Body::Point(None),
            (false, true) => Body::Collection(Collection::MultiPoint(points)),
            (true, false) => Body::Collection(Collection::MultiCurve(self.lines.into_iter().map(Curve::LineString).collect())),
            (false, false) => {
                let mut bodies: Vec<Body> = self.lines.into_iter().map(|l| Body::Curve(Curve::LineString(l))).collect();
                bodies.extend(points.into_iter().map(Body::Point));
                Body::Collection(Collection::Geometry(bodies))
            }
        }
    }
}

/// The coordinate with no z.
fn flat(p: Coord) -> Coord {
    Coord { z: 0.0, ..p }
}

/// The point of the segment `a`, `b` at the fraction `t`, measure
/// interpolated, z left out.
fn at(a: Coord, b: Coord, t: f64) -> Coord {
    let lerp = |u: f64, v: f64| (v - u).mul_add(t, u);
    Coord { x: lerp(a.x, b.x), y: lerp(a.y, b.y), z: 0.0, m: lerp(a.m, b.m) }
}

/// The part of the segment `a`, `b` whose measure is in the range, as
/// its two ends (alike for a single point); none if no part is.
fn clip(a: Coord, b: Coord, (start, end): (f64, f64)) -> Option<(Coord, Coord)> {
    if a.m.to_bits() == b.m.to_bits() {
        return (start <= a.m && a.m <= end).then(|| (flat(a), flat(b)));
    }
    let span = b.m - a.m;
    let (t0, t1) = ((start - a.m) / span, (end - a.m) / span);
    let (low, high) = (t0.min(t1).max(0.0), t0.max(t1).min(1.0));
    if low > high {
        return None;
    }
    let ends = |t: f64| {
        if t <= 0.0_f64 {
            flat(a)
        } else if t >= 1.0_f64 {
            flat(b)
        } else {
            at(a, b, t)
        }
    };
    Some((ends(low), ends(high)))
}

#[cfg(test)]
mod tests {
    use super::super::geodesy::Planar;
    use super::*;

    fn xym(x: f64, y: f64, m: f64) -> Coord {
        Coord { x, y, z: 0.0_f64, m }
    }

    fn line(points: Vec<Coord>) -> Geometry {
        Geometry { dims: Dims::Xym, body: Body::Curve(Curve::LineString(points)) }
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.15 Description
    /// 2e] A line's run between two measures is interpolated at both ends
    /// and is a multicurve; a single measure gives a multipoint.
    #[test]
    fn a_line_is_located_between_and_along() {
        let l = line(vec![xym(0.0_f64, 0.0_f64, 0.0_f64), xym(10.0_f64, 0.0_f64, 10.0_f64), xym(10.0_f64, 10.0_f64, 20.0_f64)]);
        let between = locate_between(&l, (5.0_f64, 15.0_f64), &Planar).unwrap();
        let Body::Collection(Collection::MultiCurve(runs)) = between.body else { panic!("a multicurve") };
        assert_eq!(
            runs,
            vec![Curve::LineString(vec![
                xym(5.0_f64, 0.0_f64, 5.0_f64),
                xym(10.0_f64, 0.0_f64, 10.0_f64),
                xym(10.0_f64, 5.0_f64, 15.0_f64)
            ])]
        );
        let beyond = locate_between(&l, (25.0_f64, 30.0_f64), &Planar).unwrap();
        assert_eq!(beyond.body, Body::Point(None));
        let along = locate_between(&l, (10.0_f64, 10.0_f64), &Planar).unwrap();
        assert_eq!(along.body, Body::Collection(Collection::MultiPoint(vec![Some(xym(10.0_f64, 0.0_f64, 10.0_f64))])));
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.15 Description
    /// 2a, 2c, 2d] A reversed range is 2FF02; a value with no m gives the
    /// empty point; points are kept by their m.
    #[test]
    fn ranges_unmeasured_values_and_points() {
        let l = line(vec![xym(0.0_f64, 0.0_f64, 0.0_f64), xym(1.0_f64, 0.0_f64, 1.0_f64)]);
        assert_eq!(locate_between(&l, (2.0_f64, 1.0_f64), &Planar).unwrap_err().sqlstate().as_str(), "2FF02");
        let plain = Geometry { dims: Dims::Xy, body: Body::Point(Some(Coord::xy(1.0_f64, 1.0_f64))) };
        assert_eq!(locate_between(&plain, (0.0_f64, 1.0_f64), &Planar).unwrap().body, Body::Point(None));
        let points = Collection::MultiPoint(vec![Some(xym(1.0_f64, 1.0_f64, 1.0_f64)), Some(xym(2.0_f64, 2.0_f64, 5.0_f64)), None]);
        let multi = Geometry { dims: Dims::Xym, body: Body::Collection(points) };
        let found = locate_between(&multi, (0.0_f64, 2.0_f64), &Planar).unwrap();
        assert_eq!(found.body, Body::Collection(Collection::MultiPoint(vec![Some(xym(1.0_f64, 1.0_f64, 1.0_f64))])));
        let none = locate_between(&multi, (7.0_f64, 8.0_f64), &Planar).unwrap();
        assert_eq!(none.body, Body::Point(None));
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.15 Description
    /// 2e ii 3] Runs and points apart make a collection; a run of constant
    /// measure is kept whole.
    #[test]
    fn runs_and_points_make_a_collection() {
        let up = vec![
            xym(0.0_f64, 0.0_f64, 0.0_f64),
            xym(2.0_f64, 0.0_f64, 2.0_f64),
            xym(4.0_f64, 0.0_f64, 2.0_f64),
            xym(6.0_f64, 0.0_f64, 0.0_f64),
        ];
        let found = locate_between(&line(up), (2.0_f64, 2.0_f64), &Planar).unwrap();
        let expected = vec![Curve::LineString(vec![xym(2.0_f64, 0.0_f64, 2.0_f64), xym(4.0_f64, 0.0_f64, 2.0_f64)])];
        assert_eq!(found.body, Body::Collection(Collection::MultiCurve(expected)));
        let peak = vec![xym(0.0_f64, 0.0_f64, 0.0_f64), xym(1.0_f64, 0.0_f64, 2.0_f64), xym(2.0_f64, 0.0_f64, 0.0_f64)];
        let both = Collection::Geometry(vec![Body::Curve(Curve::LineString(peak)), Body::Point(Some(xym(9.0_f64, 9.0_f64, 0.5_f64)))]);
        let at_peak = locate_between(&Geometry { dims: Dims::Xym, body: Body::Collection(both) }, (1.0_f64, 3.0_f64), &Planar).unwrap();
        let Body::Collection(Collection::MultiCurve(runs)) = at_peak.body else { panic!("a multicurve") };
        assert_eq!(runs.len(), 1);
    }
}
