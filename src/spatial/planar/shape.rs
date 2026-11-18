//! \[DESIGN\] The engine's geometry values, plain XY coordinates in the
//! shapes of ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.3 to §4.2.11,
//! with no SQL type behind them. A curve is a line string, a surface a
//! polygon; curved kinds are linearized before they reach the engine.

use super::Point;

/// A planar geometry. A ring repeats its first point last; the first
/// ring of a polygon is its exterior ring. Any variant other than
/// `Point` may hold no coordinates, and is then the empty set.
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    /// The empty set of type ST_Point, the empty result of Table 7's R00.
    Empty,
    /// One position.
    Point(Point),
    /// A line string, positions joined by line segments.
    Line(Vec<Point>),
    /// A polygon, its exterior ring and then its interior rings.
    Polygon(Vec<Vec<Point>>),
    /// Points, in no order.
    MultiPoint(Vec<Point>),
    /// Line strings.
    MultiLine(Vec<Vec<Point>>),
    /// Polygons.
    MultiPolygon(Vec<Vec<Vec<Point>>>),
    /// Any geometries.
    Collection(Vec<Geometry>),
}

/// The topological dimension of a set, ST_Dimension's value, with -1
/// for the empty set [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.2.1].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dimension {
    /// The empty set, -1.
    Empty,
    /// Points only, 0.
    Point,
    /// Curves, 1.
    Curve,
    /// Surfaces, 2.
    Surface,
}

impl Dimension {
    /// The SMALLINT value ST_Dimension returns.
    pub fn value(self) -> i8 {
        match self {
            Self::Empty => -1,
            Self::Point => 0,
            Self::Curve => 1,
            Self::Surface => 2,
        }
    }
}

/// The parts of a geometry, flattened into its points, its line strings
/// and its polygons. Every operation reads its operands through these.
#[derive(Clone, Debug, Default)]
pub(crate) struct Parts {
    pub(crate) points: Vec<Point>,
    pub(crate) lines: Vec<Vec<Point>>,
    pub(crate) polygons: Vec<Vec<Vec<Point>>>,
}

impl Parts {
    pub(crate) fn is_empty(&self) -> bool {
        self.points.is_empty() && self.lines.iter().all(Vec::is_empty) && self.polygons.iter().all(|rings| rings.iter().all(Vec::is_empty))
    }
}

impl Geometry {
    /// Whether the geometry is the empty set.
    pub fn is_empty(&self) -> bool {
        self.parts().is_empty()
    }

    /// The dimension of the geometry as its type gives it, 0 for points,
    /// 1 for curves, 2 for surfaces, a collection's greatest element's
    /// [§10.1.1 Description 5], and -1 for the empty set of type ST_Point
    /// or an empty collection.
    pub fn dimension(&self) -> Dimension {
        match self {
            Self::Empty => Dimension::Empty,
            Self::Point(_) | Self::MultiPoint(_) => Dimension::Point,
            Self::Line(_) | Self::MultiLine(_) => Dimension::Curve,
            Self::Polygon(_) | Self::MultiPolygon(_) => Dimension::Surface,
            Self::Collection(items) => items.iter().map(Geometry::dimension).max().unwrap_or(Dimension::Empty),
        }
    }

    /// The flattened parts; empty elements contribute nothing.
    pub(crate) fn parts(&self) -> Parts {
        let mut parts = Parts::default();
        self.collect(&mut parts);
        parts
    }

    fn collect(&self, parts: &mut Parts) {
        match self {
            Self::Empty => (),
            Self::Point(point) => parts.points.push(*point),
            Self::Line(line) => push_line(parts, line),
            Self::Polygon(rings) => push_polygon(parts, rings),
            Self::MultiPoint(points) => parts.points.extend(points.iter().copied()),
            Self::MultiLine(lines) => lines.iter().for_each(|line| push_line(parts, line)),
            Self::MultiPolygon(polygons) => polygons.iter().for_each(|rings| push_polygon(parts, rings)),
            Self::Collection(items) => items.iter().for_each(|item| item.collect(parts)),
        }
    }

    /// Every coordinate of the geometry, in order.
    pub(crate) fn coordinates(&self) -> Vec<Point> {
        let parts = self.parts();
        let mut all = parts.points;
        all.extend(parts.lines.into_iter().flatten());
        all.extend(parts.polygons.into_iter().flatten().flatten());
        all
    }

    /// The geometry of the most specific type for these parts. One
    /// element is itself, several of one dimension are the matching multi
    /// type, mixed dimensions a collection, and nothing the empty set.
    pub(crate) fn from_parts(parts: Parts) -> Geometry {
        let Parts { mut points, mut lines, mut polygons } = parts;
        let kinds = [points.is_empty(), lines.is_empty(), polygons.is_empty()].into_iter().filter(|empty| !empty).count();
        if kinds > 1 {
            let mut items: Vec<Geometry> = polygons.into_iter().map(Geometry::Polygon).collect();
            items.extend(lines.into_iter().map(Geometry::Line));
            items.extend(points.into_iter().map(Geometry::Point));
            return Geometry::Collection(items);
        }
        match (points.len(), lines.len(), polygons.len()) {
            (1, _, _) => points.pop().map_or(Geometry::Empty, Geometry::Point),
            (_, 1, _) => lines.pop().map_or(Geometry::Empty, Geometry::Line),
            (_, _, 1) => polygons.pop().map_or(Geometry::Empty, Geometry::Polygon),
            (0, 0, 0) => Geometry::Empty,
            (_, 0, 0) => Geometry::MultiPoint(points),
            (0, _, 0) => Geometry::MultiLine(lines),
            (_, _, _) => Geometry::MultiPolygon(polygons),
        }
    }
}

fn push_line(parts: &mut Parts, line: &[Point]) {
    if !line.is_empty() {
        parts.lines.push(line.to_vec());
    }
}

/// A polygon whose exterior ring is empty is the empty set
/// [§8.2.1 Description 17].
fn push_polygon(parts: &mut Parts, rings: &[Vec<Point>]) {
    if rings.first().is_none_or(Vec::is_empty) {
        return;
    }
    let kept: Vec<Vec<Point>> = rings.iter().filter(|ring| !ring.is_empty()).cloned().collect();
    if !kept.is_empty() {
        parts.polygons.push(kept);
    }
}
