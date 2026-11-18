//! Reads GML for ST_GMLToSQL, ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §5.1.60: the elements of Table 14, z when every position has one
//! (rule 5), no m (rule 6), and the reference system of the srsName
//! attribute (rule 7).

use super::curves::Reader;
use super::xml::{Node, invalid, parse};
use crate::error::Result;
use crate::spatial::curves::fault::{Fault, fault};
use crate::spatial::curves::tin::ElementKind;
use crate::spatial::curves::types::{Coord, Curve, Dims};
use crate::spatial::formats::tree::{Body, Collection, Element, Geometry, Ring, Rings, Surface, Tin, element_kind_of};

/// Finds the reference system an srsName names (rule 7).
pub trait Srs {
    /// The SRID of the system whose SRTEXT is `text`.
    fn by_text(&self, text: &str) -> Option<i32>;
    /// The SRID of the system `authority` numbers `code`.
    fn by_code(&self, authority: &str, code: &str) -> Option<i32>;
    /// The srsName ST_AsGML writes for `srid`: AN:AI, else SRTEXT.
    fn name(&self, srid: i32) -> Option<String>;
}

/// The SRID of `srs_name` (rule 7): 0 when absent; well-known text of a
/// reference system (it holds a `[`) by its text; `ON:OI` by authority
/// and code, the authority the last word before the last colon, so a
/// URN reads too.
///
/// # Errors
///
/// SQLSTATE 2FF59 for a name no reference system answers to.
pub fn srid(srs_name: Option<&str>, srs: &dyn Srs) -> Result<i32> {
    let Some(name) = srs_name.map(str::trim) else {
        return Ok(0);
    };
    let found = if name.contains('[') {
        srs.by_text(name)
    } else {
        name.rsplit_once(':')
            .and_then(|(before, code)| before.split(':').rev().find(|w| !w.is_empty()).and_then(|authority| srs.by_code(authority, code)))
    };
    found.ok_or_else(|| fault(Fault::UnknownSrs, "§5.1.60", &format!("no spatial reference system is named '{name}'")))
}

/// The value `text` represents and its srsName.
///
/// # Errors
///
/// SQLSTATE 2FF24 for a document that is not GML of Table 14, and the
/// curve constructors' errors for impossible arcs.
pub fn read(text: &str) -> Result<(Geometry, Option<String>)> {
    let root = parse(text)?;
    let mut r = Reader { all_z: true };
    let body = r.body(&root)?;
    // A solid is always three-dimensional (§9.1), even when empty.
    let solid = matches!(body, Body::Solid(_));
    let dims = if solid || (r.all_z && has_position(&root)) { Dims::Xyz } else { Dims::Xy };
    let body = if dims == Dims::Xy { flatten(body) } else { body };
    Ok((Geometry { dims, body }, root.attr("srsName").map(str::to_owned)))
}

/// Whether the document holds any position.
fn has_position(node: &Node) -> bool {
    matches!(node.name.as_str(), "pos" | "posList" | "coordinates" | "coord" | "location") || node.children.iter().any(has_position)
}

/// The value with every z set to zero, through the text form.
fn flatten(body: Body) -> Body {
    let g = Geometry { dims: Dims::Xy, body };
    crate::spatial::formats::wkt::read(&crate::spatial::formats::wkt::write(&g)).map_or(g.body, |back| back.body)
}

impl Reader {
    fn body(&mut self, node: &Node) -> Result<Body> {
        Ok(match node.name.as_str() {
            "Point" => Body::Point(if node.children.is_empty() { None } else { self.positions(node)?.first().copied() }),
            "PolygonPatch" | "Polygon" | "Triangle" | "PolyhedralSurface" | "Tin" | "TriangulatedSurface" | "CompositeSurface" => {
                Body::Surface(self.surface(node)?)
            }
            "Solid" => Body::Solid(self.solid(node)?),
            "MultiGeometry" => Body::Collection(Collection::Geometry(self.members(node, "geometryMember", Reader::body)?)),
            "MultiPoint" => Body::Collection(Collection::MultiPoint(self.members(node, "pointMember", Reader::point)?)),
            "MultiCurve" => Body::Collection(Collection::MultiCurve(self.members(node, "curveMember", Reader::curve)?)),
            "MultiLineString" => Body::Collection(Collection::MultiLineString(self.members(node, "lineStringMember", Reader::line)?)),
            "MultiSurface" => Body::Collection(Collection::MultiSurface(self.members(node, "surfaceMember", Reader::surface)?)),
            "MultiPolygon" => Body::Collection(Collection::MultiPolygon(self.members(node, "polygonMember", Reader::rings)?)),
            _ => Body::Curve(self.curve(node)?),
        })
    }

    fn members<T>(&mut self, node: &Node, member: &str, mut read: impl FnMut(&mut Reader, &Node) -> Result<T>) -> Result<Vec<T>> {
        let mut out = Vec::new();
        for m in node.all(member) {
            out.push(self.member(m, &mut read)?);
        }
        Ok(out)
    }

    fn point(&mut self, node: &Node) -> Result<Option<Coord>> {
        match self.body(node)? {
            Body::Point(p) => Ok(p),
            Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => Err(invalid("a pointMember holds a Point")),
        }
    }

    fn line(&mut self, node: &Node) -> Result<Ring> {
        match self.curve(node)? {
            Curve::LineString(points) => Ok(points),
            Curve::CircularString(_)
            | Curve::Circle(_)
            | Curve::GeodesicString(_)
            | Curve::Elliptical(_)
            | Curve::Nurbs(_)
            | Curve::Clothoid(_)
            | Curve::Spiral(_)
            | Curve::Compound(_) => Err(invalid("a lineStringMember holds a LineString")),
        }
    }

    /// A ring element: LinearRing's positions.
    fn linear_ring(&mut self, side: &Node) -> Result<Ring> {
        let ring = side.child("LinearRing").ok_or_else(|| invalid(&format!("<{}> holds a LinearRing", side.name)))?;
        self.positions(ring)
    }

    /// The rings of a Polygon, Triangle or PolygonPatch of linear rings.
    fn rings(&mut self, node: &Node) -> Result<Rings> {
        let sides = node.children.iter().filter(|c| c.name == "exterior" || c.name == "interior");
        sides.map(|side| self.linear_ring(side)).collect()
    }

    /// A curve ring: a Ring of curve members (one is that curve, more a
    /// compound curve), or a LinearRing.
    fn curve_ring(&mut self, side: &Node) -> Result<Curve> {
        let Some(ring) = side.child("Ring") else {
            return Ok(Curve::LineString(self.linear_ring(side)?));
        };
        let mut curves = self.members(ring, "curveMember", Reader::curve)?;
        Ok(if curves.len() == 1 { curves.remove(0) } else { Curve::Compound(curves) })
    }

    fn surface(&mut self, node: &Node) -> Result<Surface> {
        Ok(match node.name.as_str() {
            "Polygon" => Surface::Polygon(self.rings(node)?),
            "PolygonPatch" => {
                let sides = node.children.iter().filter(|c| c.name == "exterior" || c.name == "interior");
                Surface::CurvePolygon(sides.map(|side| self.curve_ring(side)).collect::<Result<_>>()?)
            }
            "Triangle" => Surface::Triangle(self.rings(node)?.into_iter().next().unwrap_or_default()),
            "PolyhedralSurface" => {
                let patches = node.child("polygonPatches").map(|p| p.children.as_slice()).unwrap_or_default();
                Surface::Polyhedral(patches.iter().map(|p| self.rings(p)).collect::<Result<_>>()?)
            }
            "Tin" | "TriangulatedSurface" => Surface::Tin(self.tin(node)?),
            "CompositeSurface" => Surface::Compound(self.members(node, "surfaceMember", Reader::surface)?),
            other => return Err(invalid(&format!("<{other}> is not a surface"))),
        })
    }

    fn tin(&mut self, node: &Node) -> Result<Tin> {
        let patches = node.child("trianglePatches").or_else(|| node.child("patches")).map(|p| p.children.as_slice()).unwrap_or_default();
        let triangles = patches.iter().map(|t| Ok(self.rings(t)?.into_iter().next().unwrap_or_default())).collect::<Result<Vec<_>>>()?;
        let mut elements = Vec::new();
        for child in &node.children {
            match child.name.as_str() {
                "element" => elements.push(self.element(child)?),
                "stopLines" | "breakLines" => {
                    let kind = if child.name == "stopLines" { ElementKind::StopLine } else { ElementKind::Breakline };
                    for line in &child.children {
                        let points = self.positions(line)?;
                        elements.push(Element { kind, id: 0, tag: String::new(), body: Body::Curve(Curve::LineString(points)) });
                    }
                }
                "controlPoint" => {
                    let points = self.positions(child)?.into_iter().map(Some).collect();
                    elements.push(Element {
                        kind: ElementKind::RandomPoints,
                        id: 0,
                        tag: String::new(),
                        body: Body::Collection(Collection::MultiPoint(points)),
                    });
                }
                _ => {}
            }
        }
        let max_side = node.child("maxLength").map(|m| super::curves::numbers(&m.text)).transpose()?.and_then(|v| v.first().copied());
        Ok(Tin { triangles, elements, max_side })
    }

    fn element(&mut self, node: &Node) -> Result<Element> {
        let name = node.attr("type").unwrap_or_default();
        let kind = ElementKind::from_name(name)
            .or_else(|| ElementKind::from_keyword(name))
            .ok_or_else(|| invalid(&format!("'{name}' is no TIN element type")))?;
        let id = node.attr("id").map_or(Ok(0_i32), |v| v.trim().parse::<i32>().map_err(|_| invalid("a TIN element's id is an integer")))?;
        let body = self.member(node, Reader::body)?;
        if body.kind() != element_kind_of(kind) {
            return Err(invalid(&format!("a {name} element holds a {}", element_kind_of(kind).keyword())));
        }
        Ok(Element { kind, id, tag: node.attr("tag").unwrap_or_default().to_owned(), body })
    }

    fn solid(&mut self, node: &Node) -> Result<Vec<Surface>> {
        let mut shells = Vec::new();
        for side in node.children.iter().filter(|c| c.name == "exterior" || c.name == "interior") {
            let shell = side.child("Shell").ok_or_else(|| invalid("a solid's boundary is a Shell"))?;
            let mut parts = self.members(shell, "surfaceMember", Reader::surface)?;
            shells.push(if parts.len() == 1 { parts.remove(0) } else { Surface::Compound(parts) });
        }
        Ok(shells)
    }
}
