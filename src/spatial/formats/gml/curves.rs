//! Reads GML curves and positions for ST_GMLToSQL, ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §5.1.60 and Table 14: line strings, Curve segments
//! of every kind the table names, composite curves, and the positions of
//! rules 3 to 5 (pos, posList, coordinates, coord).

use super::xml::{Node, invalid};
use crate::error::Result;
use crate::spatial::curves::circular::{circle_from_center, from_bulges, from_center};
use crate::spatial::curves::types::{Clothoid, Coord, Curve, Elliptical, Knot, Nurbs, NurbsPoint, Placement, Spiral, SpiralKind, Vector};
use std::f64::consts::{PI, TAU};

/// Gradians in a half turn.
const HALF_TURN_GRADIANS: f64 = 200.0;
/// The coordinates of a position with z.
const SPACE: usize = 3;

/// The reading state: whether every position so far had a z.
pub struct Reader {
    /// False once a position with two coordinates is read (rule 5).
    pub all_z: bool,
}

/// The numbers of `text`, separated by space.
///
/// # Errors
///
/// SQLSTATE 2FF24 for a word that is no number.
pub fn numbers(text: &str) -> Result<Vec<f64>> {
    text.split_whitespace().map(|w| w.parse::<f64>().map_err(|_| invalid(&format!("'{w}' is not a number")))).collect()
}

/// The one number in `node`'s text.
fn number(node: &Node) -> Result<f64> {
    let values = numbers(&node.text)?;
    values.first().copied().filter(|_| values.len() == 1).ok_or_else(|| invalid(&format!("<{}> holds one number", node.name)))
}

/// The number in child `name`.
fn field(node: &Node, name: &str) -> Result<f64> {
    number(node.child(name).ok_or_else(|| invalid(&format!("<{}> has no <{name}>", node.name)))?)
}

/// An angle in radians, by the element's uom: rad, grad or gon, else
/// degrees.
fn angle(node: &Node, name: &str) -> Result<f64> {
    let child = node.child(name).ok_or_else(|| invalid(&format!("<{}> has no <{name}>", node.name)))?;
    let value = number(child)?;
    Ok(match child.attr("uom").unwrap_or("deg") {
        "rad" => value,
        "grad" | "gon" => value * (PI / HALF_TURN_GRADIANS),
        _ => value.to_radians(),
    })
}

/// A whole number that is not negative.
fn count(value: f64) -> Result<u32> {
    format!("{value}").parse::<u32>().map_err(|_| invalid(&format!("{value} is not a count")))
}

impl Reader {
    /// One position of two or three numbers.
    fn position(&mut self, values: &[f64]) -> Result<Coord> {
        let at = |i: usize| values.get(i).copied().unwrap_or_default();
        if !(2..=SPACE).contains(&values.len()) {
            return Err(invalid("a position has two or three coordinates"));
        }
        self.all_z &= values.len() == SPACE;
        Ok(Coord::xyz(at(0), at(1), at(2)))
    }

    /// The position in `node`'s own text.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF24 for a position of another size.
    pub fn pos(&mut self, node: &Node) -> Result<Coord> {
        let values = numbers(&node.text)?;
        self.position(&values)
    }

    /// The positions a list element holds: a posList (srsDimension, else
    /// two), pos elements, a coordinates element or coord elements.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF24 for malformed positions.
    pub fn positions(&mut self, node: &Node) -> Result<Vec<Coord>> {
        if let Some(list) = node.child("posList") {
            let size = list
                .attr("srsDimension")
                .or_else(|| node.attr("srsDimension"))
                .map_or(Ok(2), |d| d.trim().parse::<usize>().map_err(|_| invalid("srsDimension is a count")))?;
            let values = numbers(&list.text)?;
            if size == 0 || !values.len().is_multiple_of(size) {
                return Err(invalid("a posList's numbers do not fill its positions"));
            }
            return values.chunks_exact(size).map(|p| self.position(p)).collect();
        }
        if let Some(coordinates) = node.child("coordinates") {
            let mut out = Vec::new();
            for tuple in coordinates.text.split_whitespace() {
                let values: Vec<f64> = tuple
                    .split(',')
                    .map(|v| v.parse::<f64>().map_err(|_| invalid("a coordinate is not a number")))
                    .collect::<Result<_>>()?;
                out.push(self.position(&values)?);
            }
            return Ok(out);
        }
        let mut out = Vec::new();
        for child in &node.children {
            match child.name.as_str() {
                "pos" => out.push(self.pos(child)?),
                "coord" => {
                    let axes: Vec<f64> = ["X", "Y", "Z"].iter().filter_map(|a| child.child(a)).map(number).collect::<Result<_>>()?;
                    out.push(self.position(&axes)?);
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// A curve element: LineString, Curve, CompositeCurve or a segment.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF24 for an element that is no curve or is malformed.
    pub fn curve(&mut self, node: &Node) -> Result<Curve> {
        match node.name.as_str() {
            "LineString" => Ok(Curve::LineString(self.positions(node)?)),
            "CompositeCurve" => Ok(Curve::Compound(node.all("curveMember").map(|m| self.member(m, Reader::curve)).collect::<Result<_>>()?)),
            "Curve" => {
                let segments = node.child("segments").map(|s| s.children.as_slice()).unwrap_or_default();
                let mut curves: Vec<Curve> = segments.iter().map(|s| self.segment(s)).collect::<Result<_>>()?;
                if curves.len() == 1 { Ok(curves.remove(0)) } else { Ok(Curve::Compound(curves)) }
            }
            _ => self.segment(node),
        }
    }

    /// The one element inside a member element, read by `read`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF24 for a member that does not hold one element.
    pub fn member<T>(&mut self, node: &Node, read: impl FnOnce(&mut Reader, &Node) -> Result<T>) -> Result<T> {
        match node.children.as_slice() {
            [only] => read(self, only),
            [] | [_, _, ..] => Err(invalid(&format!("<{}> holds one element", node.name))),
        }
    }

    fn segment(&mut self, node: &Node) -> Result<Curve> {
        let empty = node.children.is_empty();
        Ok(match node.name.as_str() {
            "LineStringSegment" => Curve::LineString(self.positions(node)?),
            "Arc" | "ArcString" => Curve::CircularString(self.positions(node)?),
            "Circle" => Curve::Circle(self.positions(node)?),
            "Geodesic" | "GeodesicString" => Curve::GeodesicString(self.positions(node)?),
            "ArcByBulge" | "ArcStringByBulge" => Curve::CircularString(self.bulges(node)?),
            "ArcByCenterPoint" => Curve::CircularString(self.centered(node, false)?),
            "CircleByCenterPoint" => Curve::Circle(self.centered(node, true)?),
            "EllipticalCurve" | "Ellipse" => Curve::Elliptical(if empty { None } else { Some(self.elliptical(node)?) }),
            "BSpline" => Curve::Nurbs(if empty { None } else { Some(self.nurbs(node)?) }),
            "Clothoid" => Curve::Clothoid(if empty { None } else { Some(self.clothoid(node)?) }),
            "SpiralCurve" => Curve::Spiral(if empty { None } else { Some(self.spiral(node)?) }),
            other => return Err(invalid(&format!("<{other}> is not a curve"))),
        })
    }

    fn bulges(&mut self, node: &Node) -> Result<Vec<Coord>> {
        let points = self.positions(node)?;
        let bulges: Vec<f64> = node.all("bulge").map(number).collect::<Result<_>>()?;
        let mut normals = Vec::new();
        for n in node.all("normal") {
            let values = numbers(&n.text)?;
            normals.push(match values.as_slice() {
                [z] => [0.0_f64, 0.0_f64, *z],
                [x, y, z] => [*x, *y, *z],
                _ => return Err(invalid("a bulge's normal has one or three components")),
            });
        }
        from_bulges(&points, &bulges, &normals)
    }

    fn centered(&mut self, node: &Node, whole: bool) -> Result<Vec<Coord>> {
        let center = self.positions(node)?.first().copied().ok_or_else(|| invalid("an arc by center point has a center"))?;
        let radius = field(node, "radius")?;
        if whole {
            return circle_from_center(center, radius, [0.0, 0.0, 1.0], false);
        }
        let start = angle(node, "startAngle")?;
        let end = angle(node, "endAngle")?;
        from_center(center, radius, start, if end < start { end + TAU } else { end })
    }

    fn placement(&mut self, node: &Node) -> Result<Placement> {
        let affine = node
            .child("refLocation")
            .and_then(|r| r.child("AffinePlacement"))
            .ok_or_else(|| invalid(&format!("<{}> has no refLocation", node.name)))?;
        let location = affine.child("location").ok_or_else(|| invalid("an affine placement has a location"))?;
        let location = if let Some(pos) = location.child("pos") { self.pos(pos)? } else { self.pos(location)? };
        let mut directions = Vec::new();
        for d in affine.all("refDirection") {
            let values = numbers(&d.text)?;
            let point = self.position(&values)?;
            directions.push(Vector::of(point.space()));
        }
        Ok(Placement { location, directions })
    }

    fn elliptical(&mut self, node: &Node) -> Result<Elliptical> {
        let placement = self.placement(node)?;
        let (start, end) = if node.name == "Ellipse" { (0.0_f64, TAU) } else { (angle(node, "startAngle")?, angle(node, "endAngle")?) };
        Ok(Elliptical { placement, u_axis: field(node, "uAxisLength")?, v_axis: field(node, "vAxisLength")?, start, end, measures: None })
    }

    fn clothoid(&mut self, node: &Node) -> Result<Clothoid> {
        let placement = self.placement(node)?;
        Ok(Clothoid {
            placement,
            scale: field(node, "scaleFactor")?,
            start: field(node, "startParameter")?,
            end: field(node, "endParameter")?,
            measures: None,
        })
    }

    fn spiral(&mut self, node: &Node) -> Result<Spiral> {
        let placement = self.placement(node)?;
        let name = node.child("spiralType").map(|t| t.text.trim()).unwrap_or_default();
        let kind = SpiralKind::named(name).ok_or_else(|| invalid(&format!("'{name}' is no spiral type")))?;
        Ok(Spiral {
            placement,
            length: field(node, "length")?,
            start_curvature: field(node, "startCurvature")?,
            end_curvature: field(node, "endCurvature")?,
            kind,
            measures: None,
        })
    }

    fn nurbs(&mut self, node: &Node) -> Result<Nurbs> {
        let points = self.positions(node)?;
        let degree = count(field(node, "degree")?)?;
        let mut knots = Vec::new();
        for k in node.all("knot") {
            let knot = k.child("Knot").ok_or_else(|| invalid("a knot holds a Knot"))?;
            let multiplicity = count(field(knot, "multiplicity")?)?;
            knots.push(Knot { value: field(knot, "value")?, multiplicity });
        }
        let weights = node.child("weights").map(|w| numbers(&w.text)).transpose()?;
        if weights.as_ref().is_some_and(|w| w.len() != points.len()) {
            return Err(invalid("a BSpline has one weight per control point"));
        }
        let points = points
            .into_iter()
            .enumerate()
            .map(|(i, point)| NurbsPoint { point, weight: weights.as_ref().and_then(|w| w.get(i).copied()) })
            .collect();
        Ok(Nurbs { degree, points, knots, measures: None })
    }
}
