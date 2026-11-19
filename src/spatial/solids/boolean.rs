//! \[DESIGN\] The 3D set operations of ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §5.1.30 to §5.1.33 on pieces, exactly. Volumes combine by
//! partitions (Thibault and Naylor, 1987): the union keeps each solid's
//! faces outside the other and their shared faces once; the intersection
//! keeps each solid's faces inside the other; the difference keeps the
//! first's faces outside the second and the second's inside the first,
//! turned. Surfaces, curves and points keep what lies in, or out of, the
//! other value, and the intersection adds where pieces of the two cross.
//! Then a lower piece inside a higher one of the result goes, so the
//! result is a point set with no part counted twice.

use super::bsp::Label;
use super::cells::Pieces;
use super::contact::{Member, Operand, Reach, polys_meet, seg_poly, segs_cross, split_in};
use super::pieces::Poly;

/// The pieces of `first ∩ second`.
pub fn intersection(first: &Operand, second: &Operand) -> Pieces {
    let mut out = Pieces::default();
    let mut volume = Vec::new();
    for face in first.pieces.solids.iter().flatten() {
        for (part, label) in second.against(face.clone()) {
            match label {
                Label::In | Label::OnSame => volume.push(part),
                Label::OnOpposite => out.polys.push(part),
                Label::Out => out.polys.extend(split_in(part, second, Reach::Surfaces).0),
            }
        }
    }
    for face in second.pieces.solids.iter().flatten() {
        volume.extend(second_inside(face, first));
    }
    out.solids = vec![volume];
    for poly in &first.pieces.polys {
        out.polys.extend(split_in(poly.clone(), second, Reach::Surfaces).0);
    }
    for poly in &second.pieces.polys {
        out.polys.extend(split_in(poly.clone(), first, Reach::Volume).0);
    }
    for seg in &first.pieces.segments {
        out.segments.extend(split_in(seg.clone(), second, Reach::Curves).0);
    }
    for seg in &second.pieces.segments {
        out.segments.extend(split_in(seg.clone(), first, Reach::Surfaces).0);
    }
    for point in &first.pieces.points {
        out.points.extend(split_in(point.clone(), second, Reach::All).0);
    }
    for point in &second.pieces.points {
        out.points.extend(split_in(point.clone(), first, Reach::Curves).0);
    }
    crossings(first, second, &mut out);
    out
}

/// The parts of `face`, of the second value's solids, strictly inside
/// the first value's.
fn second_inside(face: &Poly, first: &Operand) -> Vec<Poly> {
    first.against(face.clone()).into_iter().filter(|(_, label)| *label == Label::In).map(|(part, _)| part).collect()
}

/// Adds where pieces of `first` and `second` cross: the segments and
/// points where their surfaces and solids' faces meet, and the points
/// where curves cross them and each other.
fn crossings(first: &Operand, second: &Operand, out: &mut Pieces) {
    let faces = |o: &Operand| o.pieces.solids.iter().flatten().chain(&o.pieces.polys).cloned().collect::<Vec<Poly>>();
    let (mine, theirs) = (faces(first), faces(second));
    for a in &mine {
        for b in &theirs {
            let (segments, points) = polys_meet(a, b);
            out.segments.extend(segments);
            out.points.extend(points);
        }
    }
    for (segs, polys) in [(&first.pieces.segments, &theirs), (&second.pieces.segments, &mine)] {
        for seg in segs {
            out.points.extend(polys.iter().filter_map(|p| seg_poly(seg, p)));
        }
    }
    for a in &first.pieces.segments {
        out.points.extend(second.pieces.segments.iter().filter_map(|b| segs_cross(a, b)));
    }
}

/// The pieces of `first ∪ second`.
pub fn union(first: &Operand, second: &Operand) -> Pieces {
    let mut volume = Vec::new();
    for face in first.pieces.solids.iter().flatten() {
        let kept = second.against(face.clone()).into_iter().filter(|(_, l)| matches!(l, Label::Out | Label::OnSame));
        volume.extend(kept.map(|(part, _)| part));
    }
    for face in second.pieces.solids.iter().flatten() {
        let kept = first.against(face.clone()).into_iter().filter(|(_, l)| *l == Label::Out);
        volume.extend(kept.map(|(part, _)| part));
    }
    let mut out = Pieces { solids: vec![volume], ..Pieces::default() };
    for operand in [first, second] {
        out.polys.extend(operand.pieces.polys.iter().cloned());
        out.segments.extend(operand.pieces.segments.iter().cloned());
        out.points.extend(operand.pieces.points.iter().cloned());
    }
    out
}

/// The pieces of `first − second`, closed: a part of `first` goes where
/// it lies in a part of `second` of its dimension or higher.
pub fn difference(first: &Operand, second: &Operand) -> Pieces {
    let mut volume = Vec::new();
    for face in first.pieces.solids.iter().flatten() {
        let kept = second.against(face.clone()).into_iter().filter(|(_, l)| matches!(l, Label::Out | Label::OnOpposite));
        volume.extend(kept.map(|(part, _)| part));
    }
    for face in second.pieces.solids.iter().flatten() {
        volume.extend(second_inside(face, first).iter().map(Poly::flipped));
    }
    let mut out = Pieces { solids: vec![volume], ..Pieces::default() };
    out.polys = outside(&first.pieces.polys, second, Reach::Surfaces);
    out.segments = outside(&first.pieces.segments, second, Reach::Curves);
    out.points = outside(&first.pieces.points, second, Reach::All);
    out
}

/// The parts of `pieces` outside `operand` within `reach`.
fn outside<T: Member>(pieces: &[T], operand: &Operand, reach: Reach) -> Vec<T> {
    pieces.iter().flat_map(|p| split_in(p.clone(), operand, reach).1).collect()
}

/// `pieces` with no part counted twice: surfaces, curves and points
/// inside a higher part, or inside an earlier one of their own kind, go.
pub fn reduced(pieces: Pieces) -> Pieces {
    let volume = Operand::new(Pieces { solids: pieces.solids.clone(), ..Pieces::default() });
    let polys = distinct(pieces.polys.into_iter().flat_map(|p| split_in(p, &volume, Reach::Volume).1).collect());
    let surfaces = Operand::new(Pieces { solids: pieces.solids.clone(), polys: polys.clone(), ..Pieces::default() });
    let segments = distinct(pieces.segments.into_iter().flat_map(|s| split_in(s, &surfaces, Reach::Surfaces).1).collect());
    let curves =
        Operand::new(Pieces { solids: pieces.solids.clone(), polys: polys.clone(), segments: segments.clone(), points: Vec::new() });
    let points = distinct(pieces.points.into_iter().flat_map(|p| split_in(p, &curves, Reach::Curves).1).collect());
    let solids = pieces.solids.into_iter().filter(|s| !s.is_empty()).collect();
    Pieces { points, segments: segments.into_iter().filter(|s| !s.is_point()).collect(), polys, solids }
}

/// `pieces` with each part already held by an earlier one dropped.
fn distinct<T: Member>(pieces: Vec<T>) -> Vec<T> {
    let mut kept: Vec<T> = Vec::new();
    for piece in pieces {
        let mut left = vec![piece];
        for held in &kept {
            left = left.into_iter().flat_map(|part| part.outside_of(held)).collect();
        }
        kept.extend(left);
    }
    kept
}

/// Whether `pieces` has no part.
pub fn is_empty(pieces: &Pieces) -> bool {
    pieces.points.is_empty() && pieces.segments.is_empty() && pieces.polys.is_empty() && pieces.solids.iter().all(Vec::is_empty)
}
