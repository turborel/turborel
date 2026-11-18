//! \[DESIGN\] Constrained Delaunay triangulation in the plane, for the
//! triangulation ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.3 D4c leaves
//! implementation defined. Points go in by Bowyer-Watson insertion
//! inside a super triangle, never growing a cavity across a constrained
//! edge. Constraints go in by flipping the edges they cross (Sloan),
//! after splitting them at vertices they pass through and at earlier
//! constraints they cross; Lawson flips then restore the Delaunay
//! property everywhere a constraint allows. Directed edges map to their
//! triangle, so neighbours are found by looking up the reversed edge.

mod constrain;

use super::numeric::{HALF, V3};
use std::collections::{HashMap, HashSet, VecDeque};

/// The super triangle reaches this many times the points' extent.
const SUPER_REACH: f64 = 1e4;
/// Orientation and in-circle results within this fraction of the
/// extent's square (or fourth power) count as zero.
const DEGENERATE: f64 = 1e-12;
/// The number of super triangle vertices, which come first.
const SUPER: usize = 3;

/// An undirected edge, smaller vertex first.
pub type Edge = (usize, usize);

/// The undirected key of edge (a, b).
pub fn edge(a: usize, b: usize) -> Edge {
    if a <= b { (a, b) } else { (b, a) }
}

/// Where a point falls in the mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    /// Strictly inside the triangle.
    Inside(usize),
    /// On the edge from the first vertex to the second of the triangle.
    OnEdge(usize, usize, usize),
    /// At the vertex.
    Vertex(usize),
}

/// A triangulation under construction.
#[derive(Clone, Debug)]
pub struct Mesh {
    /// Vertices: the three super vertices, then the inserted points.
    pub points: Vec<V3>,
    triangles: Vec<Option<[usize; 3]>>,
    owners: HashMap<(usize, usize), usize>,
    constrained: HashSet<Edge>,
    last: usize,
    scale: f64,
}

impl Mesh {
    /// An empty mesh able to hold points within `low` to `high` in x and y.
    pub fn new(low: [f64; 2], high: [f64; 2]) -> Mesh {
        let [lx, ly] = low;
        let [hx, hy] = high;
        let extent = (hx - lx).max(hy - ly).max(1.0);
        let (cx, cy) = (HALF * (lx + hx), HALF * (ly + hy));
        let reach = SUPER_REACH * extent;
        let points = vec![[cx - reach, cy - reach, 0.0_f64], [cx + reach, cy - reach, 0.0_f64], [cx, cy + reach, 0.0_f64]];
        let mut mesh = Mesh { points, triangles: Vec::new(), owners: HashMap::new(), constrained: HashSet::new(), last: 0, scale: extent };
        mesh.add([0, 1, 2]);
        mesh
    }

    fn xy(&self, v: usize) -> (f64, f64) {
        self.points.get(v).map_or((0.0, 0.0), |[x, y, _]| (*x, *y))
    }

    /// Twice the signed area of a b c, zero within the tolerance.
    pub fn orient(&self, first: usize, second: usize, third: usize) -> f64 {
        let ((ax, ay), (bx, by), (cx, cy)) = (self.xy(first), self.xy(second), self.xy(third));
        let value = (bx - ax).mul_add(cy - ay, -((by - ay) * (cx - ax)));
        if value.abs() <= DEGENERATE * self.scale * self.scale { 0.0 } else { value }
    }

    /// Positive when d is inside the circle through counterclockwise a b c.
    fn in_circle(&self, triangle: [usize; 3], point: usize) -> f64 {
        let (px, py) = self.xy(point);
        let rows = triangle.map(|vertex| {
            let (vx, vy) = self.xy(vertex);
            let (dx, dy) = (vx - px, vy - py);
            [dx, dy, dx.mul_add(dx, dy * dy)]
        });
        let [[r0x, r0y, r0w], [r1x, r1y, r1w], [r2x, r2y, r2w]] = rows;
        let minor = |p: f64, q: f64, r: f64, s: f64| p.mul_add(s, -(q * r));
        let value = r0x * minor(r1y, r1w, r2y, r2w) - r0y * minor(r1x, r1w, r2x, r2w) + r0w * minor(r1x, r1y, r2x, r2y);
        let size = self.scale * self.scale;
        if value.abs() <= DEGENERATE * size * size { 0.0 } else { value }
    }

    fn add(&mut self, t: [usize; 3]) -> usize {
        let id = self.triangles.len();
        self.triangles.push(Some(t));
        for (u, v) in sides(t) {
            self.owners.insert((u, v), id);
        }
        self.last = id;
        id
    }

    fn remove(&mut self, id: usize) -> Option<[usize; 3]> {
        let t = self.triangles.get_mut(id)?.take()?;
        for (u, v) in sides(t) {
            if self.owners.get(&(u, v)) == Some(&id) {
                self.owners.remove(&(u, v));
            }
        }
        Some(t)
    }

    fn triangle(&self, id: usize) -> Option<[usize; 3]> {
        self.triangles.get(id).copied().flatten()
    }

    /// The triangle on the other side of directed edge (u, v).
    fn across(&self, u: usize, v: usize) -> Option<usize> {
        self.owners.get(&(v, u)).copied()
    }

    /// Whether edge (a, b) is in the mesh, either way round.
    pub fn has_edge(&self, a: usize, b: usize) -> bool {
        self.owners.contains_key(&(a, b)) || self.owners.contains_key(&(b, a))
    }

    /// Whether edge (a, b) is constrained.
    pub fn is_constrained(&self, a: usize, b: usize) -> bool {
        self.constrained.contains(&edge(a, b))
    }

    fn locate(&self, p: usize) -> Spot {
        let mut current = self.triangle(self.last).map(|_| self.last).or_else(|| self.live().next());
        for _ in 0..self.triangles.len().saturating_add(SUPER) {
            let Some(id) = current else { break };
            let Some(t) = self.triangle(id) else { break };
            match self.step(id, t, p) {
                Ok(spot) => return spot,
                Err(next) => current = next,
            }
        }
        self.scan(p)
    }

    /// The spot of p in triangle t, or the neighbour to walk to.
    fn step(&self, id: usize, t: [usize; 3], p: usize) -> Result<Spot, Option<usize>> {
        let mut on = None;
        for (u, v) in sides(t) {
            let side = self.orient(u, v, p);
            if side < 0.0_f64 {
                return Err(self.across(u, v));
            }
            if side <= 0.0_f64 {
                on = Some((u, v));
            }
        }
        Ok(self.refine(id, t, p, on))
    }

    fn refine(&self, id: usize, t: [usize; 3], p: usize, on: Option<(usize, usize)>) -> Spot {
        let at = self.points.get(p).copied().unwrap_or_default();
        if let Some(v) = t.into_iter().find(|v| self.points.get(*v).is_some_and(|q| same_xy(*q, at, self.scale))) {
            return Spot::Vertex(v);
        }
        on.map_or(Spot::Inside(id), |(u, v)| Spot::OnEdge(id, u, v))
    }

    /// Every triangle in turn, when walking fails on degenerate input.
    fn scan(&self, p: usize) -> Spot {
        for id in self.live() {
            if let Some(t) = self.triangle(id)
                && let Ok(spot) = self.step(id, t, p)
            {
                return spot;
            }
        }
        Spot::Inside(self.last)
    }

    fn live(&self) -> impl Iterator<Item = usize> + '_ {
        self.triangles.iter().enumerate().filter_map(|(id, t)| t.map(|_| id))
    }

    /// Inserts point `p`, its index returned; a point at an existing
    /// vertex's place returns that vertex.
    pub fn insert(&mut self, p: V3) -> usize {
        let index = self.points.len();
        self.points.push(p);
        let spot = self.locate(index);
        let seeds = match spot {
            Spot::Vertex(v) => {
                self.points.pop();
                return v;
            }
            Spot::Inside(id) => vec![id],
            Spot::OnEdge(id, u, v) => {
                self.split_constraint(u, v, index);
                self.across(u, v).into_iter().chain([id]).collect()
            }
        };
        let cavity = self.cavity(index, seeds);
        self.fill(index, &cavity);
        index
    }

    fn split_constraint(&mut self, u: usize, v: usize, p: usize) {
        if self.constrained.remove(&edge(u, v)) {
            self.constrained.insert(edge(u, p));
            self.constrained.insert(edge(p, v));
        }
    }

    /// The triangles whose circumcircles hold p, grown from `seeds`
    /// without crossing constrained edges.
    fn cavity(&self, p: usize, seeds: Vec<usize>) -> HashSet<usize> {
        let mut inside: HashSet<usize> = seeds.iter().copied().collect();
        let mut queue: VecDeque<usize> = seeds.into();
        while let Some(id) = queue.pop_front() {
            for (u, v) in self.triangle(id).map(sides).into_iter().flatten() {
                let Some(next) = self.across(u, v) else { continue };
                let open = !self.is_constrained(u, v) && !inside.contains(&next);
                if open && self.triangle(next).is_some_and(|t| self.in_circle(t, p) > 0.0_f64) {
                    inside.insert(next);
                    queue.push_back(next);
                }
            }
        }
        inside
    }

    /// Replaces the cavity by a fan of triangles from p to its rim.
    fn fill(&mut self, p: usize, cavity: &HashSet<usize>) {
        let mut rim = Vec::new();
        for id in cavity {
            for (u, v) in self.triangle(*id).map(sides).into_iter().flatten() {
                if self.across(u, v).is_none_or(|n| !cavity.contains(&n)) {
                    rim.push((u, v));
                }
            }
        }
        for id in cavity {
            self.remove(*id);
        }
        for (u, v) in rim {
            if u != p && v != p && self.orient(u, v, p) > 0.0_f64 {
                self.add([u, v, p]);
            }
        }
    }

    /// Flips edge (u, v) when its two triangles form a convex
    /// quadrilateral; returns the new diagonal.
    fn flip(&mut self, u: usize, v: usize) -> Option<Edge> {
        let (t1, t2) = (self.owners.get(&(u, v)).copied()?, self.owners.get(&(v, u)).copied()?);
        let w1 = third(self.triangle(t1)?, u, v)?;
        let w2 = third(self.triangle(t2)?, v, u)?;
        if self.orient(w1, u, w2) <= 0.0_f64 || self.orient(w2, v, w1) <= 0.0_f64 {
            return None;
        }
        self.remove(t1);
        self.remove(t2);
        self.add([w1, u, w2]);
        self.add([w2, v, w1]);
        Some(edge(w1, w2))
    }

    /// Whether flipping (u, v) would make its triangles Delaunay.
    fn illegal(&self, u: usize, v: usize) -> bool {
        let (Some(t1), Some(t2)) = (self.owners.get(&(u, v)), self.owners.get(&(v, u))) else {
            return false;
        };
        let other = self.triangle(*t2).and_then(|t| third(t, v, u));
        match (self.triangle(*t1), other) {
            (Some(t), Some(w)) => self.in_circle(t, w) > 0.0,
            (_, _) => false,
        }
    }

    /// Lawson flips until every unconstrained edge is locally Delaunay.
    pub fn legalize(&mut self) {
        let mut queue: VecDeque<Edge> = self.owners.keys().map(|(u, v)| edge(*u, *v)).collect();
        let mut budget = queue.len().saturating_mul(SUPER_FLIPS).saturating_add(SUPER_FLIPS);
        while let Some((u, v)) = queue.pop_front() {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                break;
            }
            if self.is_constrained(u, v) || !self.illegal(u, v) {
                continue;
            }
            if let Some((a, b)) = self.flip(u, v) {
                queue.extend([edge(a, u), edge(u, b), edge(b, v), edge(v, a)]);
            }
        }
    }

    /// The finished triangles, counterclockwise, over the inserted points
    /// numbered from zero, those touching the super triangle dropped.
    pub fn triangles(&self) -> Vec<[usize; 3]> {
        self.live()
            .filter_map(|id| self.triangle(id))
            .filter(|t| t.iter().all(|v| *v >= SUPER))
            .map(|t| t.map(|v| v.saturating_sub(SUPER)))
            .collect()
    }

    /// The inserted points, numbered from zero.
    pub fn vertices(&self) -> &[V3] {
        self.points.get(SUPER..).unwrap_or_default()
    }
}

/// Flips allowed per edge while legalizing, a bound on the loop.
const SUPER_FLIPS: usize = 64;

/// The triangle's directed sides.
fn sides(t: [usize; 3]) -> [(usize, usize); 3] {
    let [a, b, c] = t;
    [(a, b), (b, c), (c, a)]
}

/// The vertex of t that is not on its directed side (u, v).
fn third(t: [usize; 3], u: usize, v: usize) -> Option<usize> {
    sides(t).into_iter().find(|(a, b)| *a == u && *b == v).and_then(|_| t.into_iter().find(|w| *w != u && *w != v))
}

fn same_xy(a: V3, b: V3, scale: f64) -> bool {
    let ([ax, ay, _], [bx, by, _]) = (a, b);
    (ax - bx).hypot(ay - by) <= super::numeric::POINT_TOLERANCE * scale
}

#[cfg(test)]
mod tests;
