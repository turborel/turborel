//! Fixtures the engine's tests share, a reader and writer of the
//! well-known text of the seven XY kinds, so cases read as the standard
//! writes geometries, and a seeded case generator.

mod brute;
mod constructions;
mod measures;
mod operations;
mod oracle;
mod relationships;

use super::{Geometry, Point};

/// The geometry a well-known text spells; panics on anything else.
pub(super) fn wkt(text: &str) -> Geometry {
    let mut reader = Reader { tokens: tokens(text), at: 0 };
    let geometry = reader.geometry();
    assert_eq!(reader.at, reader.tokens.len(), "trailing text in {text}");
    geometry
}

fn tokens(text: &str) -> Vec<String> {
    let spaced = text.replace('(', " ( ").replace(')', " ) ").replace(',', " , ");
    spaced.split_whitespace().map(str::to_uppercase).collect()
}

struct Reader {
    tokens: Vec<String>,
    at: usize,
}

impl Reader {
    fn next(&mut self) -> String {
        let token = self.tokens[self.at].clone();
        self.at = self.at.checked_add(1).unwrap();
        token
    }

    fn peek(&self) -> &str {
        self.tokens.get(self.at).map_or("", String::as_str)
    }

    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> T) -> Vec<T> {
        if self.peek() == "EMPTY" {
            self.next();
            return Vec::new();
        }
        assert_eq!(self.next(), "(");
        let mut items = vec![item(self)];
        while self.peek() == "," {
            self.next();
            items.push(item(self));
        }
        assert_eq!(self.next(), ")");
        items
    }

    fn point(&mut self) -> Point {
        let x = self.next().parse().unwrap();
        let y = self.next().parse().unwrap();
        [x, y]
    }

    fn points(&mut self) -> Vec<Point> {
        self.list(Self::point)
    }

    fn rings(&mut self) -> Vec<Vec<Point>> {
        self.list(Self::points)
    }

    fn geometry(&mut self) -> Geometry {
        match self.next().as_str() {
            "POINT" => self.points().first().map_or(Geometry::Empty, |point| Geometry::Point(*point)),
            "LINESTRING" => Geometry::Line(self.points()),
            "POLYGON" => Geometry::Polygon(self.rings()),
            "MULTIPOINT" => {
                Geometry::MultiPoint(self.list(|reader| if reader.peek() == "(" { reader.points()[0] } else { reader.point() }))
            }
            "MULTILINESTRING" => Geometry::MultiLine(self.rings()),
            "MULTIPOLYGON" => Geometry::MultiPolygon(self.list(Self::rings)),
            "GEOMETRYCOLLECTION" => Geometry::Collection(self.list(Self::geometry)),
            other => panic!("unknown kind {other}"),
        }
    }
}

/// The well-known text of a geometry, coordinates as Rust prints them.
pub(super) fn text(geometry: &Geometry) -> String {
    let point = |[x, y]: &Point| format!("{x} {y}");
    let points = |points: &[Point]| list(points.iter().map(point));
    let rings = |rings: &[Vec<Point>]| list(rings.iter().map(|ring| points(ring)));
    match geometry {
        Geometry::Empty => "POINT EMPTY".to_owned(),
        Geometry::Point(at) => format!("POINT ({})", point(at)),
        Geometry::Line(line) => format!("LINESTRING {}", points(line)),
        Geometry::Polygon(polygon) => format!("POLYGON {}", rings(polygon)),
        Geometry::MultiPoint(all) => format!("MULTIPOINT {}", points(all)),
        Geometry::MultiLine(lines) => format!("MULTILINESTRING {}", rings(lines)),
        Geometry::MultiPolygon(polygons) => format!("MULTIPOLYGON {}", list(polygons.iter().map(|polygon| rings(polygon)))),
        Geometry::Collection(items) => format!("GEOMETRYCOLLECTION {}", list(items.iter().map(text))),
    }
}

fn list(items: impl Iterator<Item = String>) -> String {
    let items: Vec<String> = items.collect();
    if items.is_empty() { "EMPTY".to_owned() } else { format!("({})", items.join(", ")) }
}

/// A fixed-seed linear congruential generator (R-TEST-5).
pub(super) struct Seeded(pub(super) u64);

impl Seeded {
    /// The next value below `bound`.
    pub(super) fn below(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33).checked_rem(bound).unwrap()
    }

    /// A small integer coordinate pair in `0..=side`.
    pub(super) fn point(&mut self, side: u64) -> Point {
        let mut coordinate = || f64::from(u32::try_from(self.below(side.checked_add(1).unwrap())).unwrap());
        [coordinate(), coordinate()]
    }
}
