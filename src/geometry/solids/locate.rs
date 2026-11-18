//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.16 Description 2] What
//! of a measured value lies within a range of measures, in space: points
//! of measures within it, and of each line segment the stretch whose
//! measures, interpolated linearly with z, are within it. Stretches that
//! follow one another join into one line string; a point that a line
//! string already holds is not repeated.

use crate::geometry::model::{Body, Collection, Coords, Curve, Dims};

/// The parts found so far.
pub struct Located {
    dims: Dims,
    m: usize,
    start: f64,
    end: f64,
    lines: Vec<Coords>,
    points: Vec<Coords>,
}

impl Located {
    /// Nothing found yet in values of `dims`, whose measure is ordinate
    /// `m`, for measures from `start` to `end`.
    pub fn new(dims: Dims, m: usize, start: f64, end: f64) -> Located {
        Located { dims, m, start, end, lines: Vec::new(), points: Vec::new() }
    }

    fn measure(&self, p: &[f64]) -> f64 {
        p.get(self.m).copied().unwrap_or(f64::NAN)
    }

    /// Takes what `body` has within the range; surfaces and solids give
    /// nothing.
    pub fn walk(&mut self, body: &Body) {
        match body {
            Body::Point(p) => self.point(p),
            Body::Curve(Curve::LineString(line)) => self.line(line),
            Body::Collection(Collection::MultiPoint(points)) => points.iter().for_each(|p| self.point(p)),
            Body::Collection(Collection::MultiLineString(lines)) => lines.iter().for_each(|l| self.line(l)),
            Body::Collection(Collection::MultiCurve(curves)) => curves.iter().for_each(|c| self.walk(&Body::Curve(c.clone()))),
            Body::Collection(Collection::Geometry(parts)) => parts.iter().for_each(|p| self.walk(p)),
            Body::Curve(_)
            | Body::Surface(_)
            | Body::Solid(_)
            | Body::Collection(Collection::MultiSurface(_) | Collection::MultiPolygon(_)) => {}
        }
    }

    fn point(&mut self, p: &[f64]) {
        let m = self.measure(p);
        if !p.is_empty() && self.start <= m && m <= self.end {
            self.points.push(p.to_vec());
        }
    }

    fn line(&mut self, line: &[f64]) {
        let stride = self.dims.stride();
        let points: Vec<&[f64]> = line.chunks(stride).collect();
        let mut chain: Vec<Coords> = Vec::new();
        for pair in points.windows(2) {
            let [a, b] = pair else { continue };
            let Some((p, q)) = self.stretch(a, b) else {
                self.close(&mut chain);
                continue;
            };
            if p == q {
                self.isolated(&mut chain, p);
                continue;
            }
            if chain.last() != Some(&p) {
                self.close(&mut chain);
                chain.push(p);
            }
            chain.push(q);
        }
        self.close(&mut chain);
    }

    /// A point found alone, unless the line string in hand ends there.
    fn isolated(&mut self, chain: &mut Vec<Coords>, p: Coords) {
        if chain.last() != Some(&p) {
            self.close(chain);
            self.points.push(p);
        }
    }

    fn close(&mut self, chain: &mut Vec<Coords>) {
        let done = std::mem::take(chain);
        if done.len() > 1 {
            self.lines.push(done.concat());
        }
    }

    /// The stretch of segment `a` to `b` within the range, as its ends.
    fn stretch(&self, a: &[f64], b: &[f64]) -> Option<(Coords, Coords)> {
        let (from, to) = (self.measure(a), self.measure(b));
        if to - from == 0.0_f64 {
            let within = self.start <= from && from <= self.end;
            return within.then(|| (a.to_vec(), b.to_vec()));
        }
        let at = |m: f64| ((m - from) / (to - from)).clamp(0.0_f64, 1.0_f64);
        let (low, high) = (at(self.start), at(self.end));
        let (low, high) = (low.min(high), low.max(high));
        let inside = |m: f64| self.start <= m && m <= self.end;
        if !inside(from + (to - from) * (low + high) / 2.0_f64) {
            return None;
        }
        Some((along(a, b, low), along(a, b, high)))
    }

    /// What was found: line strings, points or both; an empty point if
    /// nothing.
    pub fn body(mut self) -> Body {
        let on_line = |p: &Coords, lines: &[Coords], stride: usize| lines.iter().any(|l| l.chunks(stride).any(|q| q == p.as_slice()));
        let stride = self.dims.stride();
        let lines = std::mem::take(&mut self.lines);
        let points: Vec<Coords> = self.points.into_iter().filter(|p| !on_line(p, &lines, stride)).collect();
        match (lines.is_empty(), points.is_empty()) {
            (true, true) => Body::Point(Vec::new()),
            (false, true) => Body::Collection(Collection::MultiLineString(lines)),
            (true, false) => Body::Collection(Collection::MultiPoint(points)),
            (false, false) => {
                let mut parts: Vec<Body> = lines.into_iter().map(|l| Body::Curve(Curve::LineString(l))).collect();
                parts.extend(points.into_iter().map(Body::Point));
                Body::Collection(Collection::Geometry(parts))
            }
        }
    }
}

/// The point a fraction `t` of the way from `a` to `b`, every ordinate
/// interpolated; an end itself at 0 or 1.
fn along(a: &[f64], b: &[f64], t: f64) -> Coords {
    if t <= 0.0_f64 {
        return a.to_vec();
    }
    if t >= 1.0_f64 {
        return b.to_vec();
    }
    a.iter().zip(b).map(|(x, y)| x + (y - x) * t).collect()
}
