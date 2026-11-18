//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] A value's well-known
//! binary representation, as ST_AsBinary gives it and as a value is
//! stored: little-endian throughout, the codes of Table 15.
//!
//! Where the grammar marks a value's part optional and lets the empty set
//! be written by leaving it out, a part that some other part may follow
//! leaves a reader nothing to tell the two apart by. So an empty value is
//! written in full instead (RULES.md D-SPATIAL-MODEL): an empty point as
//! NaN ordinates, the GEOS convention D85 reads; an empty value of a count
//! as a count of zero; an empty triangle as three points of NaN; an empty
//! elliptical curve, clothoid or spiral as its layout with every number
//! NaN and no reference direction; an empty NURBS curve as degree 0 and no
//! points and knots. A TIN's maximum side length that is null is NaN. A
//! `<bit>` is a byte, 0 or 1, and a TIN element's identifier four bytes,
//! signed, in its element's byte order.

use super::{Body, Collection, Coords, Curve, Dims, Geom, Kind, Measures, Placement, Surface, Tin, TinElement};
use super::{WKB_AFFINEPLACEMENT, WKB_Z};

/// `<little endian>` [Description ga].
pub const LITTLE_ENDIAN: u8 = 1;
/// `<big endian>` [Description fz].
pub const BIG_ENDIAN: u8 = 0;
/// The quiet NaN an empty part's numbers are written as.
pub const EMPTY_ORDINATE: u64 = 0x7ff8_0000_0000_0000;
/// [§5.1.68 `<triangle binary representation>`] A triangle is written as
/// its first three points.
const TRIANGLE_POINTS: usize = 3;

/// [§5.1.68] The well-known binary representation of `g`.
pub fn write_wkb(g: &Geom) -> Vec<u8> {
    let mut w = Writer { out: Vec::new(), dims: g.dims };
    w.body(&g.body);
    w.out
}

struct Writer {
    out: Vec<u8>,
    dims: Dims,
}

impl Writer {
    fn header(&mut self, kind: Kind) {
        self.out.push(LITTLE_ENDIAN);
        let base = kind.wkb_code().unwrap_or_default();
        // ST_BRepSolid's code is of z alone (`WKB_BREPSOLID`).
        let code = if kind == Kind::BRepSolid { base.saturating_add(WKB_Z) } else { base.saturating_add(self.dims.wkb_offset()) };
        self.uint(code);
    }

    fn uint(&mut self, n: u32) {
        self.out.extend_from_slice(&n.to_le_bytes());
    }

    fn count(&mut self, n: usize) {
        // The model's checks bound every count far below u32::MAX.
        self.uint(u32::try_from(n).unwrap_or(u32::MAX));
    }

    fn number(&mut self, n: f64) {
        self.out.extend_from_slice(&n.to_le_bytes());
    }

    fn numbers(&mut self, numbers: &[f64]) {
        for n in numbers {
            self.number(*n);
        }
    }

    fn empty(&mut self, count: usize) {
        for _ in 0..count {
            self.number(f64::from_bits(EMPTY_ORDINATE));
        }
    }

    fn points(&mut self, c: &Coords) {
        self.count(c.len().checked_div(self.dims.stride()).unwrap_or_default());
        self.numbers(c);
    }

    fn body(&mut self, body: &Body) {
        match body {
            Body::Point(c) => self.point(c),
            Body::Curve(c) => self.curve(c),
            Body::Surface(s) => self.surface(s),
            Body::Solid(shells) => {
                self.header(Kind::BRepSolid);
                self.count(shells.len());
                for s in shells {
                    self.surface(s);
                }
            }
            Body::Collection(c) => self.collection(c),
        }
    }

    fn point(&mut self, c: &Coords) {
        self.header(Kind::Point);
        if c.is_empty() { self.empty(self.dims.stride()) } else { self.numbers(c) }
    }

    fn curve(&mut self, c: &Curve) {
        self.header(c.kind());
        match c {
            Curve::LineString(p) | Curve::CircularString(p) | Curve::Circle(p) | Curve::GeodesicString(p) => self.points(p),
            Curve::Elliptical(e) => match e {
                Some(e) => {
                    self.placement(Some(&e.placement));
                    self.numbers(&[e.u_axis, e.v_axis, e.start_angle, e.end_angle]);
                    self.measures(e.measures);
                }
                None => self.empty_parametric(4),
            },
            Curve::Clothoid(c) => match c {
                Some(c) => {
                    self.placement(Some(&c.placement));
                    self.numbers(&[c.scale_factor, c.start_distance, c.end_distance]);
                    self.measures(c.measures);
                }
                None => self.empty_parametric(3),
            },
            Curve::Spiral(s) => self.spiral(s.as_ref()),
            Curve::Nurbs(n) => self.nurbs(n.as_ref()),
            Curve::Compound(parts) => {
                self.count(parts.len());
                for p in parts {
                    self.curve(p);
                }
            }
        }
    }

    fn spiral(&mut self, s: Option<&super::Spiral>) {
        let Some(s) = s else {
            self.placement(None);
            self.empty(3);
            self.letters("");
            self.empty_measures();
            return;
        };
        self.placement(Some(&s.placement));
        self.numbers(&[s.length, s.start_curvature, s.end_curvature]);
        self.letters(s.spiral_type.name());
        self.measures(s.measures);
    }

    /// An empty curve given by parameters: an empty placement, `numbers`
    /// NaN, and NaN measures of a value with m.
    fn empty_parametric(&mut self, numbers: usize) {
        self.placement(None);
        self.empty(numbers);
        self.empty_measures();
    }

    fn empty_measures(&mut self) {
        if self.dims.has_m() {
            self.empty(2);
        }
    }

    fn measures(&mut self, m: Option<Measures>) {
        if let Some(m) = m {
            self.numbers(&[m.start, m.end]);
        }
    }

    /// [§5.1.68 `<affineplacement binary representation>`]; of none, a
    /// location of NaN and no direction.
    fn placement(&mut self, p: Option<&Placement>) {
        let planar = self.dims.planar();
        self.out.push(LITTLE_ENDIAN);
        let code = if self.dims.has_z() { WKB_AFFINEPLACEMENT.saturating_add(WKB_Z) } else { WKB_AFFINEPLACEMENT };
        self.uint(code);
        if let Some(p) = p {
            self.numbers(&p.location);
            self.count(p.directions.len().checked_div(planar).unwrap_or_default());
            self.numbers(&p.directions);
        } else {
            self.empty(planar);
            self.count(0);
        }
    }

    fn letters(&mut self, text: &str) {
        // Letters are ASCII and at most 64 of them (`check`).
        self.out.push(u8::try_from(text.len()).unwrap_or(u8::MAX));
        self.out.extend_from_slice(text.as_bytes());
    }

    fn nurbs(&mut self, n: Option<&super::Nurbs>) {
        let Some(n) = n else {
            self.out.push(0);
            self.count(0);
            self.count(0);
            self.empty_measures();
            return;
        };
        self.out.push(n.degree);
        self.count(n.points.len());
        for p in &n.points {
            self.out.push(LITTLE_ENDIAN);
            self.numbers(&p.point);
            self.out.push(u8::from(p.weight.is_some()));
            if let Some(w) = p.weight {
                self.number(w);
            }
        }
        self.count(n.knots.len());
        for k in &n.knots {
            self.out.push(LITTLE_ENDIAN);
            self.number(k.value);
            self.out.push(k.multiplicity);
        }
        self.measures(n.measures);
    }

    fn polygon(&mut self, rings: &[Coords]) {
        self.header(Kind::Polygon);
        self.count(rings.len());
        for r in rings {
            self.points(r);
        }
    }

    fn triangle(&mut self, ring: &Coords) {
        self.header(Kind::Triangle);
        let stride = self.dims.stride();
        match ring.get(..TRIANGLE_POINTS.saturating_mul(stride)) {
            Some(points) => self.numbers(points),
            None => self.empty(TRIANGLE_POINTS.saturating_mul(stride)),
        }
    }

    fn surface(&mut self, s: &Surface) {
        match s {
            Surface::CurvePolygon(rings) => {
                self.header(Kind::CurvePolygon);
                self.count(rings.len());
                for r in rings {
                    self.curve(r);
                }
            }
            Surface::Polygon(rings) => self.polygon(rings),
            Surface::Triangle(ring) => self.triangle(ring),
            Surface::PolyhdrlSurface(patches) => {
                self.header(Kind::PolyhdrlSurface);
                self.count(patches.len());
                for p in patches {
                    self.polygon(p);
                }
            }
            Surface::Tin(tin) => self.tin(tin),
            Surface::Compound(parts) => {
                self.header(Kind::CompoundSurface);
                self.count(parts.len());
                for p in parts {
                    self.surface(p);
                }
            }
        }
    }

    fn tin(&mut self, tin: &Tin) {
        self.header(Kind::Tin);
        self.count(tin.triangles.len());
        for t in &tin.triangles {
            self.triangle(t);
        }
        self.count(tin.elements.len());
        for e in &tin.elements {
            self.element(e);
        }
        self.number(tin.max_side_length.unwrap_or(f64::from_bits(EMPTY_ORDINATE)));
    }

    fn element(&mut self, e: &TinElement) {
        self.out.push(LITTLE_ENDIAN);
        self.letters(&e.element_type);
        self.out.extend_from_slice(&e.id.to_le_bytes());
        self.letters(&e.tag);
        self.body(&e.geometry);
    }

    fn collection(&mut self, c: &Collection) {
        self.header(c.kind());
        match c {
            Collection::Geometry(parts) => {
                self.count(parts.len());
                for p in parts {
                    self.body(p);
                }
            }
            Collection::MultiPoint(points) => {
                self.count(points.len());
                for p in points {
                    self.point(p);
                }
            }
            Collection::MultiCurve(curves) => {
                self.count(curves.len());
                for curve in curves {
                    self.curve(curve);
                }
            }
            Collection::MultiLineString(lines) => {
                self.count(lines.len());
                for line in lines {
                    self.header(Kind::LineString);
                    self.points(line);
                }
            }
            Collection::MultiSurface(surfaces) => {
                self.count(surfaces.len());
                for s in surfaces {
                    self.surface(s);
                }
            }
            Collection::MultiPolygon(polygons) => {
                self.count(polygons.len());
                for p in polygons {
                    self.polygon(p);
                }
            }
        }
    }
}
