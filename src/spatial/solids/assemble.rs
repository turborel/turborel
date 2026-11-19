//! \[DESIGN\] A result's pieces made a geometry value. Coplanar polygons
//! facing one way merge: their edges are split where a vertex of another
//! lies on them, edges met both ways cancel, and what is left chains into
//! rings, an exterior turning as the plane faces and holes the other way.
//! A solid's merged faces group into shells by the vertices they share;
//! a shell bounding a positive volume is a solid's exterior and one
//! bounding a negative volume a cavity of the exterior around it.
//! Segments chain into line strings. The value is of the most specific
//! type that holds its parts: one kind of part alone, else a collection.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::cells::{P3, Pieces, same as same_point};
use super::exact::Scale;
use super::kernel::{Hom, Plane, Vector, cross, is_null, minus};
use super::pieces::{Poly, Seg};
use super::triangulate::{dominant, newell};
use super::{after, before, pick};
use crate::geometry::model::{Body, Collection, Curve, Rings, Surface};

/// A polygon of rounded points: its rings, exterior first, each closed.
pub type Face = Vec<Vec<P3>>;

/// The body of the result `pieces` at `scale`.
pub fn body(pieces: &Pieces, scale: Scale) -> Body {
    let mut parts: Vec<Body> = Vec::new();
    for faces in pieces.solids.iter().filter(|s| !s.is_empty()) {
        parts.extend(solids(&merge(faces, scale)).into_iter().map(Body::Solid));
    }
    let patches: Vec<Rings> = merge(&pieces.polys, scale).iter().map(rings_of).collect();
    match patches.len() {
        0 => {}
        1 => parts.extend(patches.into_iter().map(|r| Body::Surface(Surface::Polygon(r)))),
        _ => parts.push(Body::Surface(Surface::PolyhdrlSurface(patches))),
    }
    let lines = chains(&pieces.segments, scale);
    match lines.len() {
        0 => {}
        1 => parts.extend(lines.into_iter().map(|l| Body::Curve(Curve::LineString(l)))),
        _ => parts.push(Body::Collection(Collection::MultiLineString(lines))),
    }
    let points: Vec<Vec<f64>> = pieces.points.iter().map(|h| h.to_f64(scale).to_vec()).collect();
    match points.len() {
        0 => {}
        1 => parts.extend(points.into_iter().map(Body::Point)),
        _ => parts.push(Body::Collection(Collection::MultiPoint(points))),
    }
    if parts.len() == 1 {
        parts.pop().unwrap_or(Body::Collection(Collection::Geometry(Vec::new())))
    } else {
        Body::Collection(Collection::Geometry(parts))
    }
}

/// A merged face: its rings of rounded points, exterior first, closed.
pub type Merged = Vec<Vec<P3>>;

/// The polygons `polys` merged plane by plane, at `scale`.
pub fn merge(polys: &[Poly], scale: Scale) -> Vec<Merged> {
    let mut groups: Vec<Vec<&Poly>> = Vec::new();
    for poly in polys {
        let group = groups.iter_mut().find(|g| g.first().is_some_and(|first| first.support.coincides(&poly.support) == Some(true)));
        match group {
            Some(g) => g.push(poly),
            None => groups.push(vec![poly]),
        }
    }
    groups.into_iter().flat_map(|g| merge_group(&g, scale)).collect()
}

/// The faces of one plane's polygons.
fn merge_group(group: &[&Poly], scale: Scale) -> Vec<Merged> {
    let mut vertices: Vec<Hom> = Vec::new();
    let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
    let mut raw = Vec::new();
    for poly in group {
        let count = poly.vertices.len();
        for (k, plane) in poly.edges.iter().enumerate() {
            let (Some(a), Some(b)) = (poly.vertices.get(k), poly.vertices.get(after(k, count))) else { continue };
            raw.push((vertex_id(a, &mut vertices), vertex_id(b, &mut vertices), plane.clone()));
        }
    }
    for (a, b, plane) in raw {
        for (from, to) in split_edge(a, b, &plane, &vertices) {
            if !cancel(&mut edges, to, from) {
                let count = edges.entry((from, to)).or_default();
                *count = count.saturating_add(1);
            }
        }
    }
    let normal = group.first().map_or([0.0_f64; 3], |p| p.support.n.clone().map(|v| v.approximate().0));
    let rounded = |r: Vec<usize>| canonical(r.iter().filter_map(|&v| vertices.get(v)).map(|h| h.to_f64(scale)).collect());
    let rings: Vec<Vec<P3>> = rings(edges, &vertices).into_iter().map(rounded).collect();
    shape_faces(normal, rings)
}

/// A closed ring begun at its least point, x then y then z.
fn canonical(mut ring: Vec<P3>) -> Vec<P3> {
    ring.pop();
    let least = (0..ring.len()).min_by(|&i, &j| match (ring.get(i), ring.get(j)) {
        (Some(p), Some(q)) => p[0].total_cmp(&q[0]).then(p[1].total_cmp(&q[1])).then(p[2].total_cmp(&q[2])),
        _ => Ordering::Equal,
    });
    ring.rotate_left(least.unwrap_or_default());
    if let Some(&first) = ring.first() {
        ring.push(first);
    }
    ring
}

/// The id of `h` among `vertices`, added if new.
fn vertex_id(h: &Hom, vertices: &mut Vec<Hom>) -> usize {
    vertices.iter().position(|v| v.same(h)).unwrap_or_else(|| {
        vertices.push(h.clone());
        vertices.len().saturating_sub(1)
    })
}

/// Removes one edge `from`-`to`, if held; whether it was.
fn cancel(edges: &mut HashMap<(usize, usize), usize>, from: usize, to: usize) -> bool {
    let Some(count) = edges.get_mut(&(from, to)) else { return false };
    *count = count.saturating_sub(1);
    if *count == 0 {
        edges.remove(&(from, to));
    }
    true
}

/// Edge `a`-`b` on `plane`, split at the vertices strictly inside it.
fn split_edge(a: usize, b: usize, plane: &Plane, vertices: &[Hom]) -> Vec<(usize, usize)> {
    let (Some(start), Some(end)) = (vertices.get(a), vertices.get(b)) else { return vec![(a, b)] };
    let direction = minus_hom(end, start);
    let inside = |h: &Hom| {
        plane.side(h) == Ordering::Equal && h.along(start, &direction) == Ordering::Greater && end.along(h, &direction) == Ordering::Greater
    };
    let mut inner: Vec<usize> = (0..vertices.len()).filter(|&v| v != a && v != b && vertices.get(v).is_some_and(inside)).collect();
    inner.sort_by(|&x, &y| match (vertices.get(x), vertices.get(y)) {
        (Some(hx), Some(hy)) => hx.along(hy, &direction),
        _ => Ordering::Equal,
    });
    let chain: Vec<usize> = std::iter::once(a).chain(inner).chain(std::iter::once(b)).collect();
    chain.windows(2).filter_map(|w| if let [x, y] = w { Some((*x, *y)) } else { None }).collect()
}

/// The direction from `from` to `to`, scaled by a positive factor.
fn minus_hom(to: &Hom, from: &Hom) -> Vector {
    let scaled = |h: &Hom, w: &super::exact::Big| h.p.clone().map(|c| c.mul(w));
    let direction = minus(&scaled(to, &from.w), &scaled(from, &to.w));
    if from.w.mul(&to.w).sign() == Ordering::Less { direction.map(|c| c.neg()) } else { direction }
}

/// The directed `edges` chained into closed rings of vertex ids, each
/// without vertices between collinear neighbours.
fn rings(mut edges: HashMap<(usize, usize), usize>, vertices: &[Hom]) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut guard = edges.values().sum::<usize>().saturating_add(1);
    while let Some(&(start, next)) = edges.keys().min() {
        cancel(&mut edges, start, next);
        let mut ring = vec![start, next];
        let mut at = next;
        while at != start && guard > 0 {
            guard = guard.saturating_sub(1);
            let Some(&(_, to)) = edges.keys().filter(|(from, _)| *from == at).min() else { break };
            cancel(&mut edges, at, to);
            ring.push(to);
            at = to;
        }
        out.push(simplified(ring, vertices));
    }
    out.into_iter().filter(|r| r.len() >= 4).collect()
}

/// A closed ring without its vertices between two collinear neighbours.
fn simplified(mut open: Vec<usize>, vertices: &[Hom]) -> Vec<usize> {
    open.pop();
    while open.len() > 3 {
        let count = open.len();
        let at = |i: usize| open.get(i).and_then(|&v| vertices.get(v));
        let drop = (0..count).find(|&k| match (at(before(k, count)), at(k), at(after(k, count))) {
            (Some(p), Some(q), Some(r)) => is_null(&cross(&minus_hom(q, p), &minus_hom(r, p))),
            _ => false,
        });
        let Some(k) = drop else { break };
        open.remove(k);
    }
    if let Some(&first) = open.first() {
        open.push(first);
    }
    open
}

/// Rings sorted into faces: each exterior, turning as `normal` points,
/// with the holes, turning the other way, inside it.
fn shape_faces(normal: P3, rings: Vec<Vec<P3>>) -> Vec<Merged> {
    let dot = |n: P3| n[0] * normal[0] + n[1] * normal[1] + n[2] * normal[2];
    let (exteriors, holes): (Vec<Vec<P3>>, Vec<Vec<P3>>) = rings.into_iter().partition(|r| dot(newell(r)) > 0.0_f64);
    let axis = dominant(normal);
    let mut faces: Vec<Merged> = exteriors.into_iter().map(|r| vec![r]).collect();
    for hole in holes {
        let Some(point) = hole.first().copied() else { continue };
        if let Some(face) = faces.iter_mut().find(|f| f.first().is_some_and(|e| encloses(e, point, axis))) {
            face.push(hole);
        }
    }
    faces
}

/// Whether ring `ring`, seen along `axis`, encloses `p` (even-odd).
pub fn encloses(ring: &[P3], p: P3, axis: usize) -> bool {
    let flat = |q: &P3| [pick(q, after(axis, 3)), pick(q, before(axis, 3))];
    let target = flat(&p);
    let mut odd = false;
    for w in ring.windows(2) {
        let [a, b] = w else { continue };
        let (a, b) = (flat(a), flat(b));
        if (a[1] > target[1]) != (b[1] > target[1]) && target[0] < a[0] + (target[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
            odd = !odd;
        }
    }
    odd
}

/// A merged face as model rings.
fn rings_of(face: &Merged) -> Rings {
    face.iter().map(|r| r.iter().flatten().copied().collect()).collect()
}

/// The volume a set of closed faces bounds, positive when they face out.
pub fn volume(faces: &[&Merged]) -> f64 {
    let third = 1.0_f64 / 3.0_f64;
    faces
        .iter()
        .filter_map(|f| {
            let base = f.first()?.first().copied()?;
            let area = f.iter().map(|r| newell(r)).fold([0.0_f64; 3], |s, n| [s[0] + n[0], s[1] + n[1], s[2] + n[2]]);
            Some(0.5_f64 * third * (base[0] * area[0] + base[1] * area[1] + base[2] * area[2]))
        })
        .sum()
}

/// The solids of merged `faces`: shells by shared vertices, each cavity
/// in the smallest exterior whose box holds it.
fn solids(faces: &[Merged]) -> Vec<Vec<Surface>> {
    let mut exteriors: Vec<(f64, [P3; 2], Vec<Surface>)> = Vec::new();
    let mut cavities = Vec::new();
    for shell in components(faces) {
        let members: Vec<&Merged> = shell.iter().filter_map(|&k| faces.get(k)).collect();
        let size = volume(&members);
        let bounds = extent(members.iter().flat_map(|f| f.iter().flatten()));
        let surface = Surface::PolyhdrlSurface(members.iter().map(|f| rings_of(f)).collect());
        if size >= 0.0_f64 { exteriors.push((size, bounds, vec![surface])) } else { cavities.push((bounds, surface)) }
    }
    for (bounds, surface) in cavities {
        let holder = exteriors.iter_mut().filter(|(_, outer, _)| holds(outer, &bounds)).min_by(|x, y| x.0.total_cmp(&y.0));
        if let Some((_, _, held)) = holder {
            held.push(surface);
        }
    }
    exteriors.into_iter().map(|(_, _, held)| held).collect()
}

/// The faces grouped by the vertices they share.
fn components(faces: &[Merged]) -> Vec<Vec<usize>> {
    let key = |p: &P3| p.map(f64::to_bits);
    let mut owner: HashMap<[u64; 3], usize> = HashMap::new();
    let mut parent: Vec<usize> = (0..faces.len()).collect();
    for (k, face) in faces.iter().enumerate() {
        for p in face.iter().flatten() {
            let other = *owner.entry(key(p)).or_insert(k);
            let (a, b) = (root(&parent, k), root(&parent, other));
            if let Some(slot) = parent.get_mut(a.max(b)) {
                *slot = a.min(b);
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for k in 0..faces.len() {
        groups.entry(root(&parent, k)).or_default().push(k);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().collect();
    out.sort();
    out
}

/// The root of `k` in the forest `parent`.
fn root(parent: &[usize], mut k: usize) -> usize {
    for _ in 0..parent.len() {
        let up = parent.get(k).copied().unwrap_or(k);
        if up == k {
            break;
        }
        k = up;
    }
    k
}

/// The box of `points`, low then high corner.
fn extent<'p>(points: impl Iterator<Item = &'p P3>) -> [P3; 2] {
    points.fold([[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]], |[low, high], p| {
        [[low[0].min(p[0]), low[1].min(p[1]), low[2].min(p[2])], [high[0].max(p[0]), high[1].max(p[1]), high[2].max(p[2])]]
    })
}

/// Whether box `outer` holds box `inner`.
fn holds(outer: &[P3; 2], inner: &[P3; 2]) -> bool {
    (0..3).all(|k| pick(&outer[0], k) <= pick(&inner[0], k) && pick(&inner[1], k) <= pick(&outer[1], k))
}

/// The segments chained into line strings at ends two of them share,
/// with no vertex between two collinear neighbours.
fn chains(segments: &[Seg], scale: Scale) -> Vec<Vec<f64>> {
    let mut vertices: Vec<Hom> = Vec::new();
    let ends: Vec<[usize; 2]> = segments.iter().map(|s| [vertex_id(&s.a, &mut vertices), vertex_id(&s.b, &mut vertices)]).collect();
    let at = |v: usize| vertices.get(v);
    let straight = |p: usize, q: usize, r: usize| match (at(p), at(q), at(r)) {
        (Some(p), Some(q), Some(r)) => is_null(&cross(&minus_hom(q, p), &minus_hom(r, p))),
        _ => false,
    };
    chain_ids(&ends)
        .into_iter()
        .map(|line| {
            let mut kept: Vec<usize> = Vec::with_capacity(line.len());
            for (k, &v) in line.iter().enumerate() {
                let next = line.get(k.saturating_add(1)).copied();
                let bend = match (kept.last().copied(), next) {
                    (Some(prior), Some(next)) => !straight(prior, v, next),
                    _ => true,
                };
                if bend {
                    kept.push(v);
                }
            }
            kept.into_iter().filter_map(at).flat_map(|h| h.to_f64(scale)).collect()
        })
        .collect()
}

/// Segments `ends` chained into lines through the points exactly two of
/// them share.
pub fn chain(ends: &[[P3; 2]]) -> Vec<Vec<P3>> {
    let mut points: Vec<P3> = Vec::new();
    let mut id = |p: P3| {
        points.iter().position(|q| same_point(*q, p)).unwrap_or_else(|| {
            points.push(p);
            points.len().saturating_sub(1)
        })
    };
    let ids: Vec<[usize; 2]> = ends.iter().map(|[a, b]| [id(*a), id(*b)]).collect();
    chain_ids(&ids).into_iter().map(|line| line.into_iter().filter_map(|v| points.get(v).copied()).collect()).collect()
}

/// Segments of vertex ids chained into lines through the vertices
/// exactly two of them share.
fn chain_ids(ends: &[[usize; 2]]) -> Vec<Vec<usize>> {
    let mut degree: HashMap<usize, usize> = HashMap::new();
    for v in ends.iter().flatten() {
        let count = degree.entry(*v).or_default();
        *count = count.saturating_add(1);
    }
    let mut used = vec![false; ends.len()];
    let mut out = Vec::new();
    let open = |v: &usize| degree.get(v).copied() != Some(2);
    // Lines start at an end not shared by two, then cycles anywhere.
    let starts = ends.iter().enumerate().filter_map(|(k, [p, q])| match (open(p), open(q)) {
        (true, _) => Some((k, [*p, *q])),
        (false, true) => Some((k, [*q, *p])),
        (false, false) => None,
    });
    let starts: Vec<(usize, [usize; 2])> = starts.chain(ends.iter().copied().enumerate()).collect();
    for (start, [a, b]) in starts {
        if used.get(start).copied().unwrap_or(true) {
            continue;
        }
        mark(&mut used, start);
        let mut line = vec![a, b];
        for _ in 0..ends.len() {
            let Some(&tail) = line.last() else { break };
            if degree.get(&tail).copied() != Some(2) || tail == a {
                break;
            }
            let Some((k, next)) = follow(ends, &used, tail) else { break };
            mark(&mut used, k);
            line.push(next);
        }
        out.push(line);
    }
    out
}

/// An unused segment of `ends` with an end at `at`, and its other end.
fn follow(ends: &[[usize; 2]], used: &[bool], at: usize) -> Option<(usize, usize)> {
    ends.iter().enumerate().filter(|(k, _)| !used.get(*k).copied().unwrap_or(true)).find_map(|(k, [p, q])| {
        if *p == at {
            Some((k, *q))
        } else if *q == at {
            Some((k, *p))
        } else {
            None
        }
    })
}

fn mark(used: &mut [bool], k: usize) {
    if let Some(slot) = used.get_mut(k) {
        *slot = true;
    }
}
