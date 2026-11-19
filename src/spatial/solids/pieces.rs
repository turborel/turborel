//! \[DESIGN\] The pieces a 3D operation works on, in plane-based form
//! ([`super::kernel`]): points, segments and convex polygons. A segment
//! lies on the line of two planes between two points. A convex polygon
//! lies on its support plane inside the positive sides of its edge planes,
//! vertex `i` being the meet of the support with edges `i - 1` and `i`.
//! Splitting by a plane makes new points only as meets of planes already
//! there, so splits are exact and never drift.

use std::cmp::Ordering;
use std::rc::Rc;

use super::kernel::{Hom, Plane, Vector, cross, is_null, meet, minus};
use super::{after, before};

/// Where a piece lies against a plane.
#[derive(Debug)]
pub enum Side<T> {
    /// Wholly on the positive side, touching it at most.
    Front(T),
    /// Wholly on the negative side, touching it at most.
    Back(T),
    /// Across it: the positive part, then the negative part.
    Both(T, T),
    /// In it; for a polygon, whether it faces the same way.
    On(T, bool),
}

/// A piece of a 3D operation.
pub trait Piece: Sized + Clone {
    /// Where it lies against `plane`, split if across it.
    fn split(&self, plane: &Plane) -> Side<Self>;
}

impl Piece for Hom {
    fn split(&self, plane: &Plane) -> Side<Hom> {
        match plane.side(self) {
            Ordering::Greater => Side::Front(self.clone()),
            Ordering::Less => Side::Back(self.clone()),
            Ordering::Equal => Side::On(self.clone(), true),
        }
    }
}

/// A segment on the line of `line.0` and `line.1`, from `a` to `b`.
#[derive(Clone, Debug)]
pub struct Seg {
    /// Two planes whose line it lies on.
    pub line: (Rc<Plane>, Rc<Plane>),
    /// One end.
    pub a: Hom,
    /// The other end.
    pub b: Hom,
}

impl Seg {
    /// The segment from `a` to `b`, integer points, None if they are one
    /// point.
    pub fn of(a: &Vector, b: &Vector) -> Option<Seg> {
        let direction = minus(b, a);
        let mut planes = (0..3_usize).filter_map(|axis| {
            let mut unit = [super::exact::Big::zero(), super::exact::Big::zero(), super::exact::Big::zero()];
            if let Some(slot) = unit.get_mut(axis) {
                *slot = super::exact::Big::of(1);
            }
            let n = cross(&direction, &unit);
            (!is_null(&n)).then(|| Rc::new(Plane::normal_at(n, a)))
        });
        let first = planes.next()?;
        let second = planes.find(|p| !is_null(&cross(&first.n, &p.n)))?;
        Some(Seg { line: (first, second), a: Hom::of(a.clone()), b: Hom::of(b.clone()) })
    }

    /// The direction of its line.
    pub fn direction(&self) -> Vector {
        cross(&self.line.0.n, &self.line.1.n)
    }

    /// The point of its line on `plane`, if the line crosses it once.
    pub fn crossing(&self, plane: &Plane) -> Option<Hom> {
        meet(&self.line.0, &self.line.1, plane)
    }

    /// Whether it is one point.
    pub fn is_point(&self) -> bool {
        self.a.same(&self.b)
    }

    /// Whether `h`, a point of its line, lies between its ends.
    pub fn spans(&self, h: &Hom) -> bool {
        let d = self.direction();
        let from_a = self.a.along(h, &d);
        let from_b = self.b.along(h, &d);
        from_a != from_b || from_a == Ordering::Equal
    }
}

impl Piece for Seg {
    fn split(&self, plane: &Plane) -> Side<Seg> {
        let (sa, sb) = (plane.side(&self.a), plane.side(&self.b));
        let whole = || self.clone();
        match (sa, sb) {
            (Ordering::Equal, Ordering::Equal) => Side::On(whole(), true),
            (Ordering::Greater | Ordering::Equal, Ordering::Greater | Ordering::Equal) => Side::Front(whole()),
            (Ordering::Less | Ordering::Equal, Ordering::Less | Ordering::Equal) => Side::Back(whole()),
            (Ordering::Greater, Ordering::Less) | (Ordering::Less, Ordering::Greater) => {
                let Some(x) = self.crossing(plane) else { return Side::Front(whole()) };
                let first = Seg { line: self.line.clone(), a: self.a.clone(), b: x.clone() };
                let second = Seg { line: self.line.clone(), a: x, b: self.b.clone() };
                if sa == Ordering::Greater { Side::Both(first, second) } else { Side::Both(second, first) }
            }
        }
    }
}

/// A convex polygon: inside the positive side of each edge plane, on its
/// support, whose positive side it faces.
#[derive(Clone, Debug)]
pub struct Poly {
    /// The plane it lies on and faces along.
    pub support: Rc<Plane>,
    /// Its edges' planes, in order around it.
    pub edges: Vec<Rc<Plane>>,
    /// Its vertices: vertex `i` begins edge `i`.
    pub vertices: Vec<Hom>,
}

impl Poly {
    /// The triangle `a`, `b`, `c`, facing the side from which they turn
    /// counterclockwise; None if they are collinear.
    pub fn triangle(a: &Vector, b: &Vector, c: &Vector) -> Option<Poly> {
        let support = Plane::through(a, b, c);
        if support.is_degenerate() {
            return None;
        }
        let points = [a, b, c];
        let mut edges = Vec::with_capacity(3);
        for (at, from) in points.iter().enumerate() {
            let to = points.get(after(at, 3)).copied().unwrap_or(a);
            let opposite = points.get(before(at, 3)).copied().unwrap_or(a);
            let edge = Plane::normal_at(cross(&support.n, &minus(to, from)), from);
            let inside = edge.side(&Hom::of(opposite.clone())) == Ordering::Greater;
            edges.push(Rc::new(if inside { edge } else { edge.flipped() }));
        }
        let vertices = points.iter().map(|p| Hom::of((*p).clone())).collect();
        Some(Poly { support: Rc::new(support), edges, vertices })
    }

    /// The same polygon facing the other way.
    #[must_use]
    pub fn flipped(&self) -> Poly {
        let mut edges = self.edges.clone();
        let mut vertices = self.vertices.clone();
        edges.reverse();
        vertices.reverse();
        // Reversed, vertex i began edge i, so it now ends it: rotate so
        // each vertex begins its edge again.
        vertices.rotate_right(1);
        Poly { support: Rc::new(self.support.flipped()), edges, vertices }
    }

    /// The part on the positive side of `plane` and the part on its
    /// negative side, given the sides of its vertices, which differ.
    fn cut(&self, plane: &Rc<Plane>, sides: &[Ordering]) -> (Option<Poly>, Option<Poly>) {
        (self.part(plane, sides, Ordering::Greater), self.part(&Rc::new(plane.flipped()), sides, Ordering::Less))
    }

    /// The part strictly towards `keep` of the plane `bound`, which faces
    /// it.
    fn part(&self, bound: &Rc<Plane>, sides: &[Ordering], keep: Ordering) -> Option<Poly> {
        let count = self.edges.len();
        let kept = |at: usize| {
            let from = sides.get(at).copied().unwrap_or(Ordering::Equal);
            let to = sides.get(after(at, count)).copied().unwrap_or(Ordering::Equal);
            from == keep || to == keep
        };
        // The kept edges run on from the one leaving a vertex not kept.
        let start = (0..count).find(|&at| kept(at) && sides.get(at) != Some(&keep))?;
        let mut edges: Vec<Rc<Plane>> = (0..count)
            .map(|k| super::ahead(start, k, count))
            .take_while(|&at| kept(at))
            .filter_map(|at| self.edges.get(at).cloned())
            .collect();
        edges.push(bound.clone());
        let n = edges.len();
        let mut vertices = Vec::with_capacity(n);
        for at in 0..n {
            let prior = edges.get(before(at, n))?;
            let this = edges.get(at)?;
            vertices.push(self.reuse(prior, this).or_else(|| meet(&self.support, prior, this))?);
        }
        Some(Poly { support: self.support.clone(), edges, vertices })
    }

    /// The vertex already held between edges `before` and `after`.
    fn reuse(&self, first: &Rc<Plane>, second: &Rc<Plane>) -> Option<Hom> {
        let count = self.edges.len();
        (0..count).find_map(|at| {
            let prior = self.edges.get(before(at, count))?;
            let this = self.edges.get(at)?;
            (Rc::ptr_eq(prior, first) && Rc::ptr_eq(this, second)).then(|| self.vertices.get(at).cloned()).flatten()
        })
    }
}

impl Piece for Poly {
    fn split(&self, plane: &Plane) -> Side<Poly> {
        let sides: Vec<Ordering> = self.vertices.iter().map(|v| plane.side(v)).collect();
        let front = sides.contains(&Ordering::Greater);
        let back = sides.contains(&Ordering::Less);
        match (front, back) {
            (false, false) => Side::On(self.clone(), super::kernel::dot(&self.support.n, &plane.n).sign() == Ordering::Greater),
            (true, false) => Side::Front(self.clone()),
            (false, true) => Side::Back(self.clone()),
            (true, true) => match self.cut(&Rc::new(plane.clone()), &sides) {
                (Some(f), Some(b)) => Side::Both(f, b),
                (Some(f), None) => Side::Front(f),
                (None, Some(b)) => Side::Back(b),
                (None, None) => Side::Front(self.clone()),
            },
        }
    }
}
