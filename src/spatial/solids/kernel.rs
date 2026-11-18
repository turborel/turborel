//! \[DESIGN\] Plane-based exact geometry (Bernstein and Fussell, "Fast,
//! Exact, Linear Booleans", 2009). Coordinates are integers of one
//! [`Scale`]. A plane is four integers; a point is the meet of three
//! planes, kept as homogeneous integers. Every point a 3D operation makes
//! is the meet of three planes through input points, so the integers stay
//! bounded however often a piece is split, and the side of a point
//! against a plane is the exact sign of a polynomial.

use std::cmp::Ordering;

use super::exact::{Big, Scale};

/// A vector of integers.
pub type Vector = [Big; 3];

/// The plane `n . x + d = 0`; its positive side is where `n . x + d > 0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plane {
    /// The normal.
    pub n: Vector,
    /// The offset.
    pub d: Big,
}

/// The point `p / w`, `w` never zero.
#[derive(Clone, Debug)]
pub struct Hom {
    /// The coordinates times `w`.
    pub p: Vector,
    /// The weight.
    pub w: Big,
}

/// The determinant of the rows `a`, `b`, `c`.
pub fn determinant3(a: &Vector, b: &Vector, c: &Vector) -> Big {
    let minor = |p: &Big, q: &Big, r: &Big, s: &Big| p.mul(q).sub(&r.mul(s));
    let first = a[0].mul(&minor(&b[1], &c[2], &b[2], &c[1]));
    let second = a[1].mul(&minor(&b[0], &c[2], &b[2], &c[0]));
    first.sub(&second).add(&a[2].mul(&minor(&b[0], &c[1], &b[1], &c[0])))
}

/// `u - v`.
pub fn minus(u: &Vector, v: &Vector) -> Vector {
    [u[0].sub(&v[0]), u[1].sub(&v[1]), u[2].sub(&v[2])]
}

/// The cross product `u x v`.
pub fn cross(u: &Vector, v: &Vector) -> Vector {
    [u[1].mul(&v[2]).sub(&u[2].mul(&v[1])), u[2].mul(&v[0]).sub(&u[0].mul(&v[2])), u[0].mul(&v[1]).sub(&u[1].mul(&v[0]))]
}

/// The dot product `u . v`.
pub fn dot(u: &Vector, v: &Vector) -> Big {
    u[0].mul(&v[0]).add(&u[1].mul(&v[1])).add(&u[2].mul(&v[2]))
}

/// Whether every component is zero.
pub fn is_null(u: &Vector) -> bool {
    u.iter().all(Big::is_zero)
}

impl Plane {
    /// The plane through `a`, `b`, `c`, positive on the side from which
    /// they turn counterclockwise; a zero normal if they are collinear.
    pub fn through(a: &Vector, b: &Vector, c: &Vector) -> Plane {
        let normal = cross(&minus(b, a), &minus(c, a));
        let offset = dot(&normal, a).neg();
        Plane { n: normal, d: offset }
    }

    /// The plane of normal `n` through `a`.
    pub fn normal_at(n: Vector, a: &Vector) -> Plane {
        let d = dot(&n, a).neg();
        Plane { n, d }
    }

    /// The plane of normal `n` through the point `h`, positive along `n`.
    pub fn normal_through(n: &Vector, h: &Hom) -> Plane {
        let positive = h.w.sign() == Ordering::Greater;
        let w = if positive { h.w.clone() } else { h.w.neg() };
        let offset = dot(n, &h.p).neg();
        let scaled = [n[0].mul(&w), n[1].mul(&w), n[2].mul(&w)];
        Plane { n: scaled, d: if positive { offset } else { offset.neg() } }
    }

    /// The plane `x[axis] = value`, positive above it.
    pub fn axis(axis: usize, value: &Big) -> Plane {
        let mut n = [Big::zero(), Big::zero(), Big::zero()];
        if let Some(slot) = n.get_mut(axis) {
            *slot = Big::of(1);
        }
        Plane { n, d: value.neg() }
    }

    /// The same plane facing the other way.
    #[must_use]
    pub fn flipped(&self) -> Plane {
        Plane { n: [self.n[0].neg(), self.n[1].neg(), self.n[2].neg()], d: self.d.neg() }
    }

    /// Whether it is no plane: its normal is zero.
    pub fn is_degenerate(&self) -> bool {
        is_null(&self.n)
    }

    /// The side of `h`: greater on the positive side, equal on it.
    pub fn side(&self, h: &Hom) -> Ordering {
        let value = dot(&self.n, &h.p).add(&self.d.mul(&h.w)).sign();
        if h.w.sign() == Ordering::Less { value.reverse() } else { value }
    }

    /// Whether `other` is this plane or this plane facing the other way:
    /// Some(true) the same way, Some(false) the other, None if another.
    pub fn coincides(&self, other: &Plane) -> Option<bool> {
        let aligned = self.n.iter().zip(&other.n).all(|(mine, theirs)| self.d.mul(theirs).sub(&other.d.mul(mine)).is_zero());
        if !is_null(&cross(&self.n, &other.n)) || !aligned {
            return None;
        }
        Some(dot(&self.n, &other.n).sign() == Ordering::Greater)
    }
}

/// The one point of three planes, if they meet in one.
pub fn meet(p: &Plane, q: &Plane, r: &Plane) -> Option<Hom> {
    let w = determinant3(&p.n, &q.n, &r.n);
    if w.is_zero() {
        return None;
    }
    let [dp, dq, dr] = [p.d.neg(), q.d.neg(), r.d.neg()];
    let column = |axis: usize| {
        let swap = |plane: &Plane, d: &Big| {
            let mut row = plane.n.clone();
            if let Some(slot) = row.get_mut(axis) {
                *slot = d.clone();
            }
            row
        };
        determinant3(&swap(p, &dp), &swap(q, &dq), &swap(r, &dr))
    };
    Some(Hom { p: [column(0), column(1), column(2)], w })
}

impl Hom {
    /// The point of integer coordinates `p`.
    pub fn of(p: Vector) -> Hom {
        Hom { p, w: Big::of(1) }
    }

    /// Whether it is the same point as `other`.
    pub fn same(&self, other: &Hom) -> bool {
        self.p.iter().zip(&other.p).all(|(mine, theirs)| mine.mul(&other.w).sub(&theirs.mul(&self.w)).is_zero())
    }

    /// Its coordinates as floats, in the units `scale` scaled from.
    pub fn to_f64(&self, scale: Scale) -> [f64; 3] {
        [scale.ratio(&self.p[0], &self.w), scale.ratio(&self.p[1], &self.w), scale.ratio(&self.p[2], &self.w)]
    }

    /// The order of this point and `other` along `direction`.
    pub fn along(&self, other: &Hom, direction: &Vector) -> Ordering {
        let mine = dot(direction, &self.p).mul(&other.w);
        let theirs = dot(direction, &other.p).mul(&self.w);
        let order = mine.sub(&theirs).sign();
        if self.w.mul(&other.w).sign() == Ordering::Less { order.reverse() } else { order }
    }
}
