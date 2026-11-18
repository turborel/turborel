//! \[DESIGN\] Where the pieces of one value meet those of another: the
//! part of a piece that lies in a value, and the points and segments
//! where pieces of two values cross. A value's point set is its solids'
//! closed volumes, its surfaces' pieces, its segments and its points; a
//! piece lies in it where a solid's partition says inside or boundary, or
//! where it lies in a surface piece, a segment or a point of it.

use std::cmp::Ordering;
use std::rc::Rc;

use super::after;
use super::bsp::{Label, Tree};
use super::cells::Pieces;
use super::kernel::{Hom, Plane, cross, dot, meet};
use super::pieces::{Piece, Poly, Seg, Side};

/// A value made ready to meet others: its pieces and its solids'
/// partitions.
#[derive(Debug)]
pub struct Operand {
    /// Its pieces.
    pub pieces: Pieces,
    trees: Vec<Tree>,
}

/// How much of a value a piece is tested against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reach {
    /// Its solids only.
    Volume,
    /// Its solids and surfaces.
    Surfaces,
    /// Its solids, surfaces and curves.
    Curves,
    /// All of it.
    All,
}

impl Operand {
    /// The operand of `pieces`.
    pub fn new(pieces: Pieces) -> Operand {
        let trees = pieces.solids.iter().filter_map(|faces| Tree::of(faces.clone())).collect();
        Operand { pieces, trees }
    }

    /// Whether it has no part.
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// The parts of `piece` with where each lies against its solids,
    /// taken together: inside or on one, or outside all.
    pub fn against<T: Piece>(&self, piece: T) -> Vec<(T, Label)> {
        let mut done = Vec::new();
        let mut pending = vec![piece];
        for tree in &self.trees {
            let mut next = Vec::new();
            for (split, label) in pending.into_iter().flat_map(|part| tree.classify(part)) {
                if label == Label::Out { next.push(split) } else { done.push((split, label)) }
            }
            pending = next;
        }
        done.extend(pending.into_iter().map(|part| (part, Label::Out)));
        done
    }
}

/// A piece that can be tested against a value's lower parts.
pub trait Member: Piece {
    /// Of `self`, the part in `poly` (if any) and the parts outside it.
    fn in_poly(self, poly: &Poly) -> (Vec<Self>, Vec<Self>);
    /// Of `self`, the part on `seg` and the parts off it.
    fn on_seg(self, seg: &Seg) -> (Vec<Self>, Vec<Self>);
    /// Whether it is the point `h`.
    fn is_at(&self, h: &Hom) -> bool;
    /// The parts of it outside `held`, a piece of its kind.
    fn outside_of(self, held: &Self) -> Vec<Self>;
}

/// Of `piece`, the parts in `operand` within `reach`, and those outside.
pub fn split_in<T: Member>(piece: T, operand: &Operand, reach: Reach) -> (Vec<T>, Vec<T>) {
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for (part, label) in operand.against(piece) {
        if label == Label::Out { outside.push(part) } else { inside.push(part) }
    }
    let pieces = &operand.pieces;
    if reach != Reach::Volume {
        outside = sift(outside, &mut inside, pieces.polys.iter(), T::in_poly);
    }
    if matches!(reach, Reach::Curves | Reach::All) {
        outside = sift(outside, &mut inside, pieces.segments.iter(), T::on_seg);
    }
    if reach == Reach::All {
        let (at, off): (Vec<T>, Vec<T>) = outside.into_iter().partition(|part| pieces.points.iter().any(|h| part.is_at(h)));
        inside.extend(at);
        outside = off;
    }
    (inside, outside)
}

/// A test of a piece against a cell: the parts in it and those not.
type Test<T, C> = fn(T, &C) -> (Vec<T>, Vec<T>);

/// Moves the parts of `outside` that `test` finds in any of `cells` to
/// `inside`, giving those left.
fn sift<'c, T, C: 'c>(outside: Vec<T>, inside: &mut Vec<T>, cells: impl Iterator<Item = &'c C>, test: Test<T, C>) -> Vec<T> {
    let mut left = outside;
    for cell in cells {
        let mut next = Vec::new();
        for part in left {
            let (found, rest) = test(part, cell);
            inside.extend(found);
            next.extend(rest);
        }
        left = next;
    }
    left
}

/// Of `piece`, the part inside every plane of `bounds` (on one counts
/// when `closed`), and the parts cut off.
fn clip<T: Piece>(piece: T, bounds: &[Rc<Plane>], closed: bool) -> (Vec<T>, Vec<T>) {
    let mut outside = Vec::new();
    let mut current = piece;
    for plane in bounds {
        current = match current.split(plane) {
            Side::Front(front) => front,
            Side::Both(front, back) => {
                outside.push(back);
                front
            }
            Side::On(on, _) if closed => on,
            Side::Back(part) | Side::On(part, _) => {
                outside.push(part);
                return (Vec::new(), outside);
            }
        };
    }
    (vec![current], outside)
}

/// The planes that bound segment `seg` along its line, facing in.
pub fn ends(seg: &Seg) -> [Rc<Plane>; 2] {
    let line = seg.direction();
    let forward = seg.b.along(&seg.a, &line) != Ordering::Less;
    let direction = if forward { line } else { line.map(|c| c.neg()) };
    let back = [direction[0].neg(), direction[1].neg(), direction[2].neg()];
    [Rc::new(Plane::normal_through(&direction, &seg.a)), Rc::new(Plane::normal_through(&back, &seg.b))]
}

/// Whether segment `seg` lies on the line of `other`.
fn collinear(seg: &Seg, other: &Seg) -> bool {
    [&seg.a, &seg.b].iter().all(|h| other.line.0.side(h) == Ordering::Equal && other.line.1.side(h) == Ordering::Equal)
}

impl Member for Poly {
    fn in_poly(self, poly: &Poly) -> (Vec<Poly>, Vec<Poly>) {
        if self.support.coincides(&poly.support).is_none() {
            return (Vec::new(), vec![self]);
        }
        clip(self, &poly.edges, false)
    }

    fn on_seg(self, _: &Seg) -> (Vec<Poly>, Vec<Poly>) {
        (Vec::new(), vec![self])
    }

    fn is_at(&self, _: &Hom) -> bool {
        false
    }

    fn outside_of(self, held: &Poly) -> Vec<Poly> {
        self.in_poly(held).1
    }
}

impl Member for Seg {
    fn in_poly(self, poly: &Poly) -> (Vec<Seg>, Vec<Seg>) {
        if poly.support.side(&self.a) != Ordering::Equal || poly.support.side(&self.b) != Ordering::Equal {
            return (Vec::new(), vec![self]);
        }
        clip(self, &poly.edges, true)
    }

    fn on_seg(self, seg: &Seg) -> (Vec<Seg>, Vec<Seg>) {
        if !collinear(&self, seg) {
            return (Vec::new(), vec![self]);
        }
        clip(self, &ends(seg), true)
    }

    fn is_at(&self, _: &Hom) -> bool {
        false
    }

    fn outside_of(self, held: &Seg) -> Vec<Seg> {
        self.on_seg(held).1
    }
}

impl Member for Hom {
    fn in_poly(self, poly: &Poly) -> (Vec<Hom>, Vec<Hom>) {
        if inside_poly(&self, poly) { (vec![self], Vec::new()) } else { (Vec::new(), vec![self]) }
    }

    fn on_seg(self, seg: &Seg) -> (Vec<Hom>, Vec<Hom>) {
        let on = seg.line.0.side(&self) == Ordering::Equal && seg.line.1.side(&self) == Ordering::Equal && seg.spans(&self);
        if on { (vec![self], Vec::new()) } else { (Vec::new(), vec![self]) }
    }

    fn is_at(&self, h: &Hom) -> bool {
        self.same(h)
    }

    fn outside_of(self, held: &Hom) -> Vec<Hom> {
        if self.same(held) { Vec::new() } else { vec![self] }
    }
}

/// Whether point `h` lies in the closed polygon `poly`.
pub fn inside_poly(h: &Hom, poly: &Poly) -> bool {
    poly.support.side(h) == Ordering::Equal && poly.edges.iter().all(|e| e.side(h) != Ordering::Less)
}

/// Where a polygon meets a plane across it: a segment or a point.
pub enum Section {
    /// A segment of the plane's line with the polygon's support.
    Segment(Seg),
    /// One point.
    Point(Hom),
}

/// Where `poly` meets `plane`, which it does not lie in; None if it does
/// not.
pub fn section(poly: &Poly, plane: &Rc<Plane>) -> Option<Section> {
    let sides: Vec<Ordering> = poly.vertices.iter().map(|v| plane.side(v)).collect();
    let count = sides.len();
    let mut points: Vec<Hom> = Vec::new();
    for (at, side) in sides.iter().enumerate() {
        let next = sides.get(after(at, count)).copied().unwrap_or(Ordering::Equal);
        let crossing = match (side, next) {
            (Ordering::Equal, _) => poly.vertices.get(at).cloned(),
            (Ordering::Greater, Ordering::Less) | (Ordering::Less, Ordering::Greater) => {
                poly.edges.get(at).and_then(|edge| meet(&poly.support, edge, plane))
            }
            (Ordering::Greater | Ordering::Less, Ordering::Equal | Ordering::Greater | Ordering::Less) => None,
        };
        if let Some(h) = crossing
            && !points.iter().any(|p| p.same(&h))
        {
            points.push(h);
        }
    }
    match points.as_slice() {
        [] => None,
        [h] => Some(Section::Point(h.clone())),
        [a, b, ..] => Some(Section::Segment(Seg { line: (poly.support.clone(), plane.clone()), a: a.clone(), b: b.clone() })),
    }
}

/// Where convex polygons `first` and `second`, not coplanar, meet: the
/// segments and points they share.
pub fn polys_meet(first: &Poly, second: &Poly) -> (Vec<Seg>, Vec<Hom>) {
    if first.support.coincides(&second.support).is_some() {
        return (Vec::new(), Vec::new());
    }
    match section(first, &second.support) {
        None => (Vec::new(), Vec::new()),
        Some(Section::Point(h)) => (Vec::new(), if inside_poly(&h, second) { vec![h] } else { Vec::new() }),
        Some(Section::Segment(seg)) => {
            let (inside, _) = clip(seg, &second.edges, true);
            let (segs, points) = inside.into_iter().partition(|s| !s.is_point());
            (segs, points.into_iter().map(|s: Seg| s.a).collect())
        }
    }
}

/// Where segment `seg` crosses polygon `poly` whose plane it does not
/// lie in: the one point, if any.
pub fn seg_poly(seg: &Seg, poly: &Poly) -> Option<Hom> {
    let (sa, sb) = (poly.support.side(&seg.a), poly.support.side(&seg.b));
    let h = match (sa, sb) {
        (Ordering::Equal, Ordering::Equal) | (Ordering::Greater, Ordering::Greater) | (Ordering::Less, Ordering::Less) => return None,
        (Ordering::Equal, _) => seg.a.clone(),
        (_, Ordering::Equal) => seg.b.clone(),
        (Ordering::Greater, Ordering::Less) | (Ordering::Less, Ordering::Greater) => seg.crossing(&poly.support)?,
    };
    inside_poly(&h, poly).then_some(h)
}

/// Where segments `first` and `second`, not collinear, cross: the one
/// point, if any.
pub fn segs_cross(first: &Seg, second: &Seg) -> Option<Hom> {
    if collinear(first, second) || is_parallel(first, second) {
        return None;
    }
    let plane = [&second.line.0, &second.line.1].into_iter().find(|p| dot(&first.direction(), &p.n).sign() != Ordering::Equal)?;
    let h = first.crossing(plane)?;
    let on_second = second.line.0.side(&h) == Ordering::Equal && second.line.1.side(&h) == Ordering::Equal;
    (on_second && first.spans(&h) && second.spans(&h)).then_some(h)
}

/// Whether the lines of two segments are parallel.
fn is_parallel(first: &Seg, second: &Seg) -> bool {
    super::kernel::is_null(&cross(&first.direction(), &second.direction()))
}
