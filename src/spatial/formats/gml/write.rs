//! Writes GML for ST_AsGML, ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §5.1.61, by Table 14. The m coordinates are left out (rule 4). An
//! empty value is its element with no content; curves other than line
//! strings are a Curve of one segment; elliptical curves, spirals,
//! NURBS weights and TIN elements use the extension elements the reader
//! takes back.

use super::xml::escape;
use crate::spatial::curves::types::{Clothoid, Coord, Curve, Dims, Elliptical, Nurbs, Placement, Spiral};
use crate::spatial::formats::tree::{Body, Collection, Element, Geometry, Ring, Rings, Surface, Tin};
use std::fmt::Write as _;

/// The GML 3.2 namespace.
pub const NAMESPACE: &str = "http://www.opengis.net/gml/3.2";

/// The GML of `value`, its document element carrying `srs_name` when
/// given.
pub fn write(value: &Geometry, srs_name: Option<&str>) -> String {
    let mut attrs = format!(" xmlns:gml=\"{NAMESPACE}\"");
    if let Some(name) = srs_name {
        let _ = write!(attrs, " srsName=\"{}\"", escape(name));
    }
    let mut w = Writer { out: String::new(), z: value.dims.has_z(), root: Some(attrs) };
    w.body(&value.body);
    w.out
}

struct Writer {
    out: String,
    z: bool,
    /// The document element's attributes, until it is written.
    root: Option<String>,
}

impl Writer {
    /// `<gml:name>`, the document element's attributes on the first.
    fn open(&mut self, name: &str) {
        let attrs = self.root.take().unwrap_or_default();
        let _ = write!(self.out, "<gml:{name}{attrs}>");
    }

    fn close(&mut self, name: &str) {
        let _ = write!(self.out, "</gml:{name}>");
    }

    /// `<gml:name/>`, an empty value.
    fn empty(&mut self, name: &str) {
        let attrs = self.root.take().unwrap_or_default();
        let _ = write!(self.out, "<gml:{name}{attrs}/>");
    }

    /// `<gml:name>text</gml:name>`.
    fn leaf(&mut self, name: &str, text: &str) {
        let _ = write!(self.out, "<gml:{name}>{text}</gml:{name}>");
    }

    fn numbers(values: &[f64]) -> String {
        values.iter().map(|v| format!("{v}")).collect::<Vec<_>>().join(" ")
    }

    fn position(&self, p: Coord) -> Vec<f64> {
        if self.z { vec![p.x, p.y, p.z] } else { vec![p.x, p.y] }
    }

    fn pos_list(&mut self, points: &[Coord]) {
        let values: Vec<f64> = points.iter().flat_map(|p| self.position(*p)).collect();
        let dimension = if self.z { 3_i32 } else { 2_i32 };
        let _ = write!(self.out, "<gml:posList srsDimension=\"{dimension}\">{}</gml:posList>", Writer::numbers(&values));
    }

    /// An element holding one position list, or empty.
    fn listed(&mut self, name: &str, points: &[Coord]) {
        if points.is_empty() {
            self.empty(name);
        } else {
            self.open(name);
            self.pos_list(points);
            self.close(name);
        }
    }

    /// `<gml:member>` around each item.
    fn members<T>(&mut self, name: &str, member: &str, items: &[T], mut item: impl FnMut(&mut Writer, &T)) {
        if items.is_empty() {
            self.empty(name);
            return;
        }
        self.open(name);
        for each in items {
            self.open(member);
            item(self, each);
            self.close(member);
        }
        self.close(name);
    }

    fn body(&mut self, body: &Body) {
        match body {
            Body::Point(None) => self.empty("Point"),
            Body::Point(Some(p)) => {
                self.open("Point");
                let text = Writer::numbers(&self.position(*p));
                self.leaf("pos", &text);
                self.close("Point");
            }
            Body::Curve(c) => self.curve(c),
            Body::Surface(s) => self.surface(s),
            Body::Solid(shells) => self.solid(shells),
            Body::Collection(c) => self.collection(c),
        }
    }

    fn collection(&mut self, c: &Collection) {
        match c {
            Collection::Geometry(items) => self.members("MultiGeometry", "geometryMember", items, Writer::body),
            Collection::MultiPoint(points) => self.members("MultiPoint", "pointMember", points, |w, p| w.body(&Body::Point(*p))),
            Collection::MultiCurve(curves) => self.members("MultiCurve", "curveMember", curves, Writer::curve),
            Collection::MultiLineString(lines) => {
                self.members("MultiLineString", "lineStringMember", lines, |w, l| w.listed("LineString", l));
            }
            Collection::MultiSurface(surfaces) => self.members("MultiSurface", "surfaceMember", surfaces, Writer::surface),
            Collection::MultiPolygon(polygons) => self.members("MultiPolygon", "polygonMember", polygons, |w, p| w.polygon("Polygon", p)),
        }
    }

    fn curve(&mut self, c: &Curve) {
        match c {
            Curve::LineString(points) => self.listed("LineString", points),
            Curve::Compound(parts) => self.members("CompositeCurve", "curveMember", parts, Writer::curve),
            Curve::CircularString(_)
            | Curve::Circle(_)
            | Curve::GeodesicString(_)
            | Curve::Elliptical(_)
            | Curve::Nurbs(_)
            | Curve::Clothoid(_)
            | Curve::Spiral(_) => {
                self.open("Curve");
                self.open("segments");
                self.segment(c);
                self.close("segments");
                self.close("Curve");
            }
        }
    }

    fn segment(&mut self, c: &Curve) {
        match c {
            Curve::LineString(p) => self.listed("LineStringSegment", p),
            Curve::CircularString(p) => self.listed("ArcString", p),
            Curve::Circle(p) => self.listed("Circle", p),
            Curve::GeodesicString(p) => self.listed("GeodesicString", p),
            Curve::Elliptical(e) => self.parametric("EllipticalCurve", e.as_ref(), Writer::elliptical),
            Curve::Nurbs(n) => self.parametric("BSpline", n.as_ref(), Writer::nurbs),
            Curve::Clothoid(k) => self.parametric("Clothoid", k.as_ref(), Writer::clothoid),
            Curve::Spiral(s) => self.parametric("SpiralCurve", s.as_ref(), Writer::spiral),
            Curve::Compound(parts) => {
                for part in parts {
                    self.segment(part);
                }
            }
        }
    }

    fn parametric<T>(&mut self, name: &str, value: Option<&T>, body: impl FnOnce(&mut Writer, &T)) {
        let Some(found) = value else {
            self.empty(name);
            return;
        };
        self.open(name);
        body(self, found);
        self.close(name);
    }

    fn number(&mut self, name: &str, value: f64) {
        self.leaf(name, &format!("{value}"));
    }

    fn placement(&mut self, p: &Placement) {
        self.open("refLocation");
        self.open("AffinePlacement");
        let location = Writer::numbers(&self.position(p.location));
        self.leaf("location", &location);
        for d in &p.directions {
            let text = Writer::numbers(&self.position(Coord::xyz(d.x, d.y, d.z)));
            self.leaf("refDirection", &text);
        }
        let _ = write!(
            self.out,
            "<gml:inDimension>{}</gml:inDimension><gml:outDimension>{}</gml:outDimension>",
            p.directions.len(),
            if self.z { 3_i32 } else { 2_i32 }
        );
        self.close("AffinePlacement");
        self.close("refLocation");
    }

    fn elliptical(&mut self, e: &Elliptical) {
        self.placement(&e.placement);
        self.number("uAxisLength", e.u_axis);
        self.number("vAxisLength", e.v_axis);
        let _ =
            write!(self.out, "<gml:startAngle uom=\"rad\">{}</gml:startAngle><gml:endAngle uom=\"rad\">{}</gml:endAngle>", e.start, e.end);
    }

    fn clothoid(&mut self, k: &Clothoid) {
        self.placement(&k.placement);
        self.number("scaleFactor", k.scale);
        self.number("startParameter", k.start);
        self.number("endParameter", k.end);
    }

    fn spiral(&mut self, s: &Spiral) {
        self.placement(&s.placement);
        self.number("length", s.length);
        self.number("startCurvature", s.start_curvature);
        self.number("endCurvature", s.end_curvature);
        self.leaf("spiralType", s.kind.name());
    }

    fn nurbs(&mut self, n: &Nurbs) {
        let points: Vec<Coord> = n.points.iter().map(|p| p.point).collect();
        self.pos_list(&points);
        let _ = write!(self.out, "<gml:degree>{}</gml:degree>", n.degree);
        for k in &n.knots {
            let _ = write!(
                self.out,
                "<gml:knot><gml:Knot><gml:value>{}</gml:value><gml:multiplicity>{}</gml:multiplicity><gml:weight>1</gml:weight></gml:Knot></gml:knot>",
                k.value, k.multiplicity
            );
        }
        if n.points.iter().any(|p| p.weight.is_some()) {
            let weights: Vec<f64> = n.points.iter().map(|p| p.weight.unwrap_or(1.0_f64)).collect();
            self.leaf("weights", &Writer::numbers(&weights));
        }
    }

    fn ring(&mut self, side: &str, points: &Ring) {
        self.open(side);
        self.listed("LinearRing", points);
        self.close(side);
    }

    fn polygon(&mut self, name: &str, rings: &Rings) {
        let Some((exterior, interiors)) = rings.split_first() else {
            self.empty(name);
            return;
        };
        self.open(name);
        self.ring("exterior", exterior);
        for interior in interiors {
            self.ring("interior", interior);
        }
        self.close(name);
    }

    fn curve_ring(&mut self, side: &str, curve: &Curve) {
        self.open(side);
        self.open("Ring");
        self.open("curveMember");
        self.curve(curve);
        self.close("curveMember");
        self.close("Ring");
        self.close(side);
    }

    fn surface(&mut self, s: &Surface) {
        match s {
            Surface::CurvePolygon(rings) => {
                let Some((exterior, interiors)) = rings.split_first() else {
                    self.empty("PolygonPatch");
                    return;
                };
                self.open("PolygonPatch");
                self.curve_ring("exterior", exterior);
                for interior in interiors {
                    self.curve_ring("interior", interior);
                }
                self.close("PolygonPatch");
            }
            Surface::Polygon(rings) => self.polygon("Polygon", rings),
            Surface::Triangle(ring) => self.polygon("Triangle", &if ring.is_empty() { Vec::new() } else { vec![ring.clone()] }),
            Surface::Polyhedral(patches) => {
                if self.patches("PolyhedralSurface", "polygonPatches", patches, |w, p| w.polygon("PolygonPatch", p)) {
                    self.close("PolyhedralSurface");
                }
            }
            Surface::Tin(tin) => self.tin(tin),
            Surface::Compound(parts) => self.members("CompositeSurface", "surfaceMember", parts, Writer::surface),
        }
    }

    /// The patches inside `holder`, leaving element `name` open; false
    /// when there are none and `name` was written empty.
    fn patches<T>(&mut self, name: &str, holder: &str, items: &[T], mut item: impl FnMut(&mut Writer, &T)) -> bool {
        if items.is_empty() {
            self.empty(name);
            return false;
        }
        self.open(name);
        self.open(holder);
        for each in items {
            item(self, each);
        }
        self.close(holder);
        true
    }

    fn tin(&mut self, tin: &Tin) {
        if !self.patches("Tin", "trianglePatches", &tin.triangles, |w, t| w.polygon("Triangle", &vec![t.clone()])) {
            return;
        }
        for element in &tin.elements {
            self.element(element);
        }
        if let Some(side) = tin.max_side {
            let _ = write!(self.out, "<gml:maxLength uom=\"m\">{side}</gml:maxLength>");
        }
        self.close("Tin");
    }

    fn element(&mut self, e: &Element) {
        let _ = write!(self.out, "<gml:element type=\"{}\" id=\"{}\" tag=\"{}\">", escape(e.kind.name()), e.id, escape(&e.tag));
        self.body(&e.body);
        self.close("element");
    }

    fn solid(&mut self, shells: &[Surface]) {
        let Some((exterior, interiors)) = shells.split_first() else {
            self.empty("Solid");
            return;
        };
        self.open("Solid");
        for (side, shell) in std::iter::once(("exterior", exterior)).chain(interiors.iter().map(|s| ("interior", s))) {
            self.open(side);
            self.open("Shell");
            self.open("surfaceMember");
            self.surface(shell);
            self.close("surfaceMember");
            self.close("Shell");
            self.close(side);
        }
        self.close("Solid");
    }
}

/// The dimensions a written document reads back with: z kept, m dropped.
pub fn read_back_dims(dims: Dims) -> Dims {
    if dims.has_z() { Dims::Xyz } else { Dims::Xy }
}
