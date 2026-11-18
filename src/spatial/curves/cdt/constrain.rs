//! Inserting constrained edges into a [`Mesh`].

use super::{Edge, Mesh, edge};
use crate::error::Result;
use crate::spatial::curves::fault::{Fault, fault};
use crate::spatial::curves::numeric::lerp;
use std::collections::VecDeque;

impl Mesh {
    /// Makes the segment from vertex `a` to vertex `b` an edge of the
    /// mesh that no flip removes. It is split where it passes through a
    /// vertex, and where it crosses an earlier constraint a vertex is
    /// added, its z taken from the earlier constraint.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF74 when the edges crossing the segment cannot be
    /// flipped away, which only degenerate input causes.
    pub fn constrain(&mut self, a: usize, b: usize) -> Result<()> {
        let mut pending = vec![(a, b)];
        let mut budget = self.points.len().saturating_mul(self.points.len()).saturating_add(super::SUPER_FLIPS);
        while let Some((from, to)) = pending.pop() {
            budget = budget.saturating_sub(1);
            if from == to || budget == 0 {
                continue;
            }
            if let Some(v) = self.through(from, to) {
                pending.extend([(from, v), (v, to)]);
                continue;
            }
            if let Some(((c, d), t)) = self.crossing_constraint(from, to) {
                let x = self.cut((c, d), t);
                pending.extend([(c, x), (x, d), (from, x), (x, to)]);
                continue;
            }
            self.flip_through(from, to)?;
            self.constrained.insert(edge(from, to));
        }
        Ok(())
    }

    /// The vertex nearest `a` lying on the open segment a b.
    fn through(&self, a: usize, b: usize) -> Option<usize> {
        let (ax, ay) = self.xy(a);
        let (bx, by) = self.xy(b);
        let length = (bx - ax).hypot(by - ay);
        let mut best: Option<(f64, usize)> = None;
        for v in super::SUPER..self.points.len() {
            if v == a || v == b || self.orient(a, b, v).abs() > 0.0_f64 {
                continue;
            }
            let (vx, vy) = self.xy(v);
            let t = (vx - ax).mul_add(bx - ax, (vy - ay) * (by - ay)) / (length * length);
            let inside = t > 0.0_f64 && t < 1.0_f64;
            if inside && best.is_none_or(|(s, _)| t < s) {
                best = Some((t, v));
            }
        }
        best.map(|(_, v)| v)
    }

    /// A constrained edge the open segment a b properly crosses, and the
    /// fraction along that edge where it does.
    fn crossing_constraint(&self, a: usize, b: usize) -> Option<(Edge, f64)> {
        self.constrained.iter().copied().find_map(|(c, d)| {
            let shares = c == a || c == b || d == a || d == b;
            if shares || !self.crosses((a, b), (c, d)) {
                return None;
            }
            let (s, t) = (self.orient(a, b, c), self.orient(a, b, d));
            Some(((c, d), s / (s - t)))
        })
    }

    /// Whether segments p q and r s cross at a point inside both.
    pub(super) fn crosses(&self, first: Edge, second: Edge) -> bool {
        let ((p, q), (r, s)) = (first, second);
        let (o1, o2) = (self.orient(p, q, r), self.orient(p, q, s));
        let (o3, o4) = (self.orient(r, s, p), self.orient(r, s, q));
        o1 * o2 < 0.0 && o3 * o4 < 0.0
    }

    /// Splits constraint c d at fraction t with a new vertex there.
    fn cut(&mut self, constraint: Edge, t: f64) -> usize {
        let (c, d) = constraint;
        self.constrained.remove(&edge(c, d));
        let (pc, pd) = (self.points.get(c).copied().unwrap_or_default(), self.points.get(d).copied().unwrap_or_default());
        let mut point = [0.0_f64; 3];
        for ((slot, x), y) in point.iter_mut().zip(pc).zip(pd) {
            *slot = lerp(x, y, t);
        }
        self.insert(point)
    }

    /// Flips the edges crossing segment a b until it is an edge (Sloan):
    /// each crossing edge in turn is flipped when its quadrilateral is
    /// convex and otherwise waits its turn again; a new diagonal that
    /// still crosses joins the queue.
    fn flip_through(&mut self, a: usize, b: usize) -> Result<()> {
        let mut queue: VecDeque<Edge> = self.owners.keys().copied().filter(|(u, v)| u < v && self.crosses((a, b), (*u, *v))).collect();
        let mut stalled = 0_usize;
        while let Some((u, v)) = queue.pop_front() {
            if let Some(diagonal) = self.flip(u, v) {
                stalled = 0;
                if self.crosses((a, b), diagonal) {
                    queue.push_back(diagonal);
                }
                continue;
            }
            stalled = stalled.saturating_add(1);
            if stalled > queue.len() {
                break;
            }
            queue.push_back((u, v));
        }
        if self.has_edge(a, b) {
            Ok(())
        } else {
            Err(fault(Fault::InvalidGeometry, "§8.6.3", "a TIN element's edge could not be fitted into the triangulation"))
        }
    }
}
