//! Writes the canonical well-known text representation, ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §5.1.67. Keywords are upper case, a
//! nested keyword repeats the outermost `<z m>` where the grammar allows
//! one (rule ai), numbers take their shortest exact decimal form, a NaN
//! number or point is `EMPTY`, and angles are in radians.
//!
//! [`write_simple`] writes the form of Simple feature access instead [OGC
//! 06-103r4 §7.2], which differs only for a triangle, a polyhedral surface
//! and a TIN: a triangle as the `<polygon text>` of its ring, the others'
//! patches as `<polygon text>`s with no PATCHES, a TIN's elements and
//! maximum side length left out, as that form has none (RULES.md D136).

use crate::spatial::curves::types::{Clothoid, Coord, Curve, Dims, Elliptical, Measures, Nurbs, Placement, Spiral, Vector, without_m};
use crate::spatial::formats::tree::{Body, Collection, Element, Geometry, Kind, Ring, Rings, Surface, Tin};
use std::fmt::Write as _;

/// The text of `value`.
pub fn write(value: &Geometry) -> String {
    let mut w = Writer { out: String::new(), dims: value.dims, simple: false };
    w.body(&value.body);
    w.out
}

/// The text of `value` in the form of Simple feature access [OGC
/// 06-103r4 §7.2].
pub fn write_simple(value: &Geometry) -> String {
    let mut w = Writer { out: String::new(), dims: value.dims, simple: true };
    w.body(&value.body);
    w.out
}

struct Writer {
    out: String,
    dims: Dims,
    /// Whether the form is Simple feature access's.
    simple: bool,
}

impl Writer {
    fn put(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn number(&mut self, value: f64) {
        if value.is_nan() {
            self.put("EMPTY");
        } else {
            let _ = write!(self.out, "{value}");
        }
    }

    /// A type keyword and the value's `<z m>`.
    fn keyword(&mut self, kind: Kind) {
        self.put(kind.keyword());
        self.put(match self.dims {
            Dims::Xy => " ",
            Dims::Xyz => " Z ",
            Dims::Xym => " M ",
            Dims::Xyzm => " ZM ",
        });
    }

    /// `word` and, when the value has z, `<just z>`.
    fn just_z(&mut self, word: &str) {
        self.put(word);
        self.put(if self.dims.has_z() { " Z " } else { " " });
    }

    /// `EMPTY` when `items` is empty, else `(item, ...)`.
    fn list<T>(&mut self, items: &[T], mut item: impl FnMut(&mut Writer, &T)) {
        if items.is_empty() {
            self.put("EMPTY");
            return;
        }
        self.put("(");
        for (index, each) in items.iter().enumerate() {
            if index > 0 {
                self.put(", ");
            }
            item(self, each);
        }
        self.put(")");
    }

    fn point(&mut self, p: Coord, dims: Dims) {
        let values = match dims {
            Dims::Xy => vec![p.x, p.y],
            Dims::Xyz => vec![p.x, p.y, p.z],
            Dims::Xym => vec![p.x, p.y, p.m],
            Dims::Xyzm => vec![p.x, p.y, p.z, p.m],
        };
        for (index, value) in values.into_iter().enumerate() {
            if index > 0 {
                self.put(" ");
            }
            self.number(value);
        }
    }

    /// `<point text>`: `EMPTY` for no point or a NaN one.
    fn point_text(&mut self, p: Option<Coord>, dims: Dims) {
        match p.filter(|q| !q.x.is_nan()) {
            Some(q) => {
                self.put("(");
                self.point(q, dims);
                self.put(")");
            }
            None => self.put("EMPTY"),
        }
    }

    fn points(&mut self, ring: &Ring) {
        let dims = self.dims;
        self.list(ring, |w, p| w.point(*p, dims));
    }

    fn body(&mut self, body: &Body) {
        self.keyword(body.kind());
        match body {
            Body::Point(p) => self.point_text(*p, self.dims),
            Body::Curve(c) => self.curve_body(c),
            Body::Surface(s) => self.surface_body(s),
            Body::Solid(shells) => self.list(shells, Writer::surface_text),
            Body::Collection(c) => self.collection(c),
        }
    }

    fn collection(&mut self, c: &Collection) {
        match c {
            Collection::Geometry(items) => self.list(items, Writer::body),
            Collection::MultiPoint(points) => {
                let dims = self.dims;
                self.list(points, |w, p| w.point_text(*p, dims));
            }
            Collection::MultiCurve(curves) => self.list(curves, Writer::curve_text),
            Collection::MultiLineString(lines) => self.list(lines, Writer::points),
            Collection::MultiSurface(surfaces) => self.list(surfaces, Writer::surface_text),
            Collection::MultiPolygon(polygons) => self.list(polygons, Writer::polygon),
        }
    }

    /// `<curve text>`: a line string bare, any other with its keyword.
    fn curve_text(&mut self, c: &Curve) {
        if let Curve::LineString(points) = c {
            self.points(points);
        } else {
            self.keyword(c.kind());
            self.curve_body(c);
        }
    }

    fn curve_body(&mut self, c: &Curve) {
        match c {
            Curve::LineString(p) | Curve::CircularString(p) | Curve::Circle(p) | Curve::GeodesicString(p) => self.points(p),
            Curve::Elliptical(e) => self.optional(e.as_ref(), Writer::elliptical),
            Curve::Nurbs(n) => self.optional(n.as_ref(), Writer::nurbs),
            Curve::Clothoid(k) => self.optional(k.as_ref(), Writer::clothoid),
            Curve::Spiral(s) => self.optional(s.as_ref(), Writer::spiral),
            Curve::Compound(parts) => self.list(parts, Writer::curve_text),
        }
    }

    /// `EMPTY`, or `(` the body `)`.
    fn optional<T>(&mut self, value: Option<&T>, body: impl FnOnce(&mut Writer, &T)) {
        let Some(found) = value else {
            self.put("EMPTY");
            return;
        };
        self.put("(");
        body(self, found);
        self.put(")");
    }

    fn field(&mut self, word: &str, value: f64) {
        self.put(", ");
        self.put(word);
        self.put(" ");
        self.number(value);
    }

    fn measures(&mut self, measures: Measures) {
        if let Some([start, end]) = measures {
            self.field("STARTM", start);
            self.field("ENDM", end);
        }
    }

    fn placement(&mut self, placement: &Placement) {
        self.put("REFERENCELOCATION ");
        self.just_z("AFFINEPLACEMENT");
        if placement.location.x.is_nan() && placement.directions.is_empty() {
            self.put("EMPTY");
            return;
        }
        self.put("(");
        self.just_z("LOCATION");
        self.point_text(Some(placement.location), without_m(self.dims));
        self.put(", REFERENCEDIRECTIONS ");
        self.list(&placement.directions, Writer::vector);
        self.put(")");
    }

    fn vector(&mut self, v: &Vector) {
        self.just_z("VECTOR");
        self.point_text(Some(Coord::xyz(v.x, v.y, v.z)), without_m(self.dims));
    }

    fn angle(&mut self, word: &str, value: f64) {
        self.put(", ");
        self.put(word);
        self.put(" ANGLE RADIANS (");
        self.number(value);
        self.put(")");
    }

    fn elliptical(&mut self, e: &Elliptical) {
        self.placement(&e.placement);
        self.field("UAXISLENGTH", e.u_axis);
        self.field("VAXISLENGTH", e.v_axis);
        self.angle("STARTANGLE", e.start);
        self.angle("ENDANGLE", e.end);
        self.measures(e.measures);
    }

    fn clothoid(&mut self, k: &Clothoid) {
        self.placement(&k.placement);
        self.field("SCALEFACTOR", k.scale);
        self.field("STARTDISTANCE", k.start);
        self.field("ENDDISTANCE", k.end);
        self.measures(k.measures);
    }

    fn spiral(&mut self, s: &Spiral) {
        self.placement(&s.placement);
        self.field("LENGTH", s.length);
        self.field("STARTCURVATURE", s.start_curvature);
        self.field("ENDCURVATURE", s.end_curvature);
        self.put(", SPIRALTYPE ");
        self.put(s.kind.name());
        self.measures(s.measures);
    }

    fn nurbs(&mut self, n: &Nurbs) {
        let _ = write!(self.out, "DEGREE {}, ", n.degree);
        self.just_z("CONTROLPOINTS");
        let flat = without_m(self.dims);
        self.list(&n.points, |w, p| {
            w.put("NURBSPOINT (");
            w.just_z("WEIGHTEDPOINT");
            w.point_text(Some(p.point), flat);
            w.field("WEIGHT", p.weight.unwrap_or(1.0_f64));
            w.put(")");
        });
        self.put(", KNOTS ");
        self.list(&n.knots, |w, k| {
            w.put("KNOT (VALUE ");
            w.number(k.value);
            let _ = write!(w.out, ", MULTIPLICITY {})", k.multiplicity);
        });
        self.measures(n.measures);
    }

    fn polygon(&mut self, rings: &Rings) {
        self.list(rings, Writer::points);
    }

    /// `<triangle text>`: the ring's first three points; of Simple feature
    /// access, the `<polygon text>` of the ring.
    fn triangle(&mut self, ring: &Ring) {
        if self.simple {
            let rings = if ring.is_empty() { Vec::new() } else { vec![ring.clone()] };
            self.polygon(&rings);
            return;
        }
        let corners = ring.get(..3).unwrap_or_default();
        let dims = self.dims;
        self.list(corners, |w, p| w.point(*p, dims));
    }

    /// `<surface text>` or `<shell text>`: a polygon bare, any other with
    /// its keyword and no `<z m>`.
    fn surface_text(&mut self, s: &Surface) {
        if let Surface::Polygon(rings) = s {
            self.polygon(rings);
            return;
        }
        self.put(s.kind().keyword());
        self.put(" ");
        self.surface_body(s);
    }

    fn surface_body(&mut self, s: &Surface) {
        match s {
            Surface::CurvePolygon(rings) => self.list(rings, Writer::curve_text),
            Surface::Polygon(rings) => self.polygon(rings),
            Surface::Triangle(ring) => self.triangle(ring),
            Surface::Polyhedral(patches) => self.polyhedral(patches),
            Surface::Tin(tin) => self.tin(tin),
            Surface::Compound(parts) => self.list(parts, Writer::surface_text),
        }
    }

    /// A polyhedral surface's patches: `(PATCHES (POLYGON ..., ...))`, or
    /// of Simple feature access `(<polygon text>, ...)`.
    fn polyhedral(&mut self, patches: &[Rings]) {
        if self.simple {
            self.list(patches, Writer::polygon);
            return;
        }
        self.patches(patches, |w, p| {
            w.keyword(Kind::Polygon);
            w.polygon(p);
        });
    }

    /// `EMPTY` or `(PATCHES (patch, ...))`.
    fn patches<T>(&mut self, items: &[T], patch: impl FnMut(&mut Writer, &T)) {
        if items.is_empty() {
            self.put("EMPTY");
            return;
        }
        self.put("(PATCHES ");
        self.list(items, patch);
        self.put(")");
    }

    fn tin(&mut self, tin: &Tin) {
        if self.simple {
            self.list(&tin.triangles, Writer::triangle);
            return;
        }
        if tin.triangles.is_empty() {
            self.put("EMPTY");
            return;
        }
        self.put("(PATCHES ");
        self.list(&tin.triangles, Writer::triangle);
        if !tin.elements.is_empty() {
            self.put(" ELEMENTS ");
            self.list(&tin.elements, Writer::element);
        }
        if let Some(side) = tin.max_side {
            self.put(" MAXSIDELENGTH ");
            self.number(side);
        }
        self.put(")");
    }

    fn element(&mut self, e: &Element) {
        let _ = write!(self.out, "{} ID {} ", e.kind.keyword(), e.id);
        if !e.tag.is_empty() {
            let _ = write!(self.out, "TAG \"{}\" ", e.tag);
        }
        self.body(&e.body);
    }
}
