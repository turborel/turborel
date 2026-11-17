//! Deterministic geometry fixtures and an integer-only intersection
//! oracle independent of the measured geometry implementation. Rings
//! have bounded coordinates, so all integer cross products fit in i64.

type Point = [i64; 2];
type Ring = Vec<Point>;

#[derive(Clone, Copy)]
pub enum Distribution {
    Uniform,
    Clustered,
}

#[derive(Clone, Copy)]
pub enum Shape {
    Holed,
    Comb,
}

pub struct Workload {
    pub features: usize,
    pub regions: usize,
    pub distribution: Distribution,
    pub shape: Shape,
    pub lines: bool,
}

impl Workload {
    pub fn features(&self) -> Vec<Feature> {
        (0..self.features)
            .map(|n| {
                let from = position(n, self.distribution);
                if !self.lines {
                    return Feature::Point(from);
                }
                let to = if n.is_multiple_of(10) {
                    position(n + 12_347, self.distribution)
                } else {
                    [from[0] + 160, from[1] + 80 - i64::try_from(n % 161).unwrap()]
                };
                Feature::Line(from, to)
            })
            .collect()
    }

    pub fn regions(&self) -> Vec<Polygon> {
        (0..self.regions)
            .map(|n| {
                let center = position(n * 307 + 53, self.distribution);
                let rings = match self.shape {
                    Shape::Holed => vec![diamond(center, 300), diamond(center, 100)],
                    Shape::Comb => vec![comb(center)],
                };
                Polygon::new(rings)
            })
            .collect()
    }

    pub fn label(&self) -> String {
        let distribution = match self.distribution {
            Distribution::Uniform => "uniform",
            Distribution::Clustered => "clustered",
        };
        let shape = match self.shape {
            Shape::Holed => "holed diamonds",
            Shape::Comb => "128-edge combs",
        };
        let feature = if self.lines { "lines" } else { "points" };
        format!("{} {distribution} {feature}, {} {shape}", self.features, self.regions)
    }
}

fn position(n: usize, distribution: Distribution) -> Point {
    let x = i64::try_from((n * 7919 % 100_000) % 1000).unwrap() * 8;
    let y = i64::try_from(((n * 37 + 17) * 7919 % 100_000) / 100).unwrap() * 8;
    match distribution {
        Distribution::Uniform => [x, y],
        Distribution::Clustered => {
            if n.is_multiple_of(5) {
                [x, y]
            } else {
                let center = if n.is_multiple_of(2) { 2000 } else { 6000 };
                [center + x / 20, center + y / 20]
            }
        }
    }
}

fn diamond([x, y]: Point, radius: i64) -> Ring {
    vec![[x - radius, y], [x, y - radius], [x + radius, y], [x, y + radius], [x - radius, y]]
}

fn comb([x, y]: Point) -> Ring {
    let mut ring = vec![[x, y], [x + 500, y], [x + 500, y + 256]];
    for step in 1..=125_i64 {
        ring.push([x + 500 - step * 4, y + if step % 2 == 0 { 256 } else { 128 }]);
    }
    ring.push([x, y]);
    ring
}

pub enum Feature {
    Point(Point),
    Line(Point, Point),
}

impl Feature {
    pub fn wkt(&self) -> String {
        match self {
            Self::Point([x, y]) => format!("POINT ({x} {y})"),
            Self::Line([x, y], [u, v]) => format!("LINESTRING ({x} {y}, {u} {v})"),
        }
    }

    fn bounds(&self) -> (Point, Point) {
        match self {
            Self::Point(point) => (*point, *point),
            Self::Line([x, y], [u, v]) => ([(*x).min(*u), (*y).min(*v)], [(*x).max(*u), (*y).max(*v)]),
        }
    }
}

pub struct Polygon {
    rings: Vec<Ring>,
    bounds: (Point, Point),
}

impl Polygon {
    fn new(rings: Vec<Ring>) -> Self {
        let mut lower = [i64::MAX; 2];
        let mut upper = [i64::MIN; 2];
        for &[x, y] in &rings[0] {
            lower = [lower[0].min(x), lower[1].min(y)];
            upper = [upper[0].max(x), upper[1].max(y)];
        }
        Self { rings, bounds: (lower, upper) }
    }

    pub fn wkt(&self) -> String {
        let rings: Vec<_> = self
            .rings
            .iter()
            .map(|ring| format!("({})", ring.iter().map(|[x, y]| format!("{x} {y}")).collect::<Vec<_>>().join(", ")))
            .collect();
        format!("POLYGON ({})", rings.join(", "))
    }

    fn contains(&self, point: Point) -> bool {
        match location(point, &self.rings[0]) {
            Location::Outside => false,
            Location::Boundary => true,
            Location::Inside => !self.rings.iter().skip(1).any(|hole| matches!(location(point, hole), Location::Inside)),
        }
    }

    pub fn intersects(&self, feature: &Feature) -> bool {
        let (lower, upper) = feature.bounds();
        let (a, b) = self.bounds;
        if lower[0] > b[0] || lower[1] > b[1] || upper[0] < a[0] || upper[1] < a[1] {
            return false;
        }
        match feature {
            Feature::Point(point) => self.contains(*point),
            Feature::Line(from, to) => {
                self.contains(*from)
                    || self.contains(*to)
                    || self.rings.iter().any(|ring| ring.windows(2).any(|edge| crossing(*from, *to, edge[0], edge[1])))
            }
        }
    }
}

enum Location {
    Outside,
    Boundary,
    Inside,
}

fn cross([ax, ay]: Point, [bx, by]: Point, [px, py]: Point) -> i64 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

fn on_segment(a: Point, b: Point, p: Point) -> bool {
    cross(a, b, p) == 0 && (0..2).all(|axis| p[axis] >= a[axis].min(b[axis]) && p[axis] <= a[axis].max(b[axis]))
}

fn crossing(a: Point, b: Point, c: Point, d: Point) -> bool {
    if on_segment(a, b, c) || on_segment(a, b, d) || on_segment(c, d, a) || on_segment(c, d, b) {
        return true;
    }
    cross(a, b, c).signum() * cross(a, b, d).signum() < 0 && cross(c, d, a).signum() * cross(c, d, b).signum() < 0
}

fn location(point: Point, ring: &Ring) -> Location {
    let mut inside = false;
    for edge in ring.windows(2) {
        let (a, b) = (edge[0], edge[1]);
        if on_segment(a, b, point) {
            return Location::Boundary;
        }
        if (a[1] > point[1]) != (b[1] > point[1]) {
            let side = cross(a, b, point);
            if (side > 0) == (b[1] > a[1]) {
                inside = !inside;
            }
        }
    }
    if inside { Location::Inside } else { Location::Outside }
}
