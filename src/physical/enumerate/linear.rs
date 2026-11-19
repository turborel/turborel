//! \[DESIGN\] Search space linearization (Neumann and Radke, "Adaptive
//! Optimization of Very Large Join Queries", 2018, §4.2, Fig. 5, Fig. 6;
//! RULES.md D304): the relations are put in one order, that of the best
//! left-deep tree IKKBZ finds of a spanning tree of least selectivities,
//! and dynamic programming then joins only runs of that order, each split
//! in two runs, in O(n³). A linear optimal tree is found exactly, and a
//! bushy one is never dearer than the best linear one.
//!
//! IKKBZ (Ibaraki and Kameda 1984; Krishnamurthy, Boral and Zaniolo 1986,
//! as the paper gives it) roots the tree at each relation in turn; below
//! each node it merges its children's chains by rank, `(T - 1) / C`, the
//! cost against the benefit of a join under Cout, and joins a node with
//! the head of its chain into one compound while the head's rank is lower,
//! so each chain is in ascending rank; the root's chain of least cost is
//! the order. Of a compound of `a` then `b`, `T = T(a) T(b)` and
//! `C = C(a) + T(a) C(b)`.

use super::bits::{Set, bit, members};
use super::space::Space;

/// A module of a chain: its relations, in order, and its T and C.
#[derive(Debug, Clone)]
struct Module {
    rels: Vec<usize>,
    t: f64,
    c: f64,
}

impl Module {
    /// Its rank, (T - 1) / C.
    fn rank(&self) -> f64 {
        if self.c > 0.0_f64 { (self.t - 1.0_f64) / self.c } else { f64::NEG_INFINITY }
    }

    /// `self` then `next`, as one compound.
    fn then(mut self, next: Module) -> Module {
        let (t, c) = (self.t * next.t, self.c + self.t * next.c);
        self.rels.extend(next.rels);
        Module { rels: self.rels, t, c }
    }
}

impl Space<'_> {
    /// The plans of every run of the IKKBZ order of `u`'s relations, if
    /// simple edges span them.
    pub(super) fn linearized(&mut self, u: Set) -> bool {
        let Some(order) = self.ikkbz(u) else { return false };
        let n = order.len();
        let run = |from: usize, to: usize| order.get(from..to).unwrap_or_default().iter().fold(0, |s, &r| s | bit(r));
        for len in 2..=n {
            for i in 0..=n.saturating_sub(len) {
                for j in 1..len {
                    let (l, r) = (run(i, i.saturating_add(j)), run(i.saturating_add(j), i.saturating_add(len)));
                    if self.has(l) && self.has(r) {
                        self.join(l, r);
                    }
                }
            }
        }
        true
    }

    /// The fraction of pairs of relations `a` and `b` their join keeps.
    fn fraction(&self, a: usize, b: usize) -> f64 {
        let (sa, sb) = (bit(a), bit(b));
        let Some(applied) = self.applies(sa, sb) else { return 1.0_f64 };
        let (ra, rb) = (self.model.rows(a), self.model.rows(b));
        let joined = self.model.joined_rows((sa, ra), (sb, rb), &applied);
        if ra * rb > 0.0_f64 { joined / (ra * rb) } else { 1.0_f64 }
    }

    /// Of the relations of `u`, the spanning tree of least selectivities
    /// (Prim's), as each relation's neighbours in it; none if simple edges
    /// do not span them.
    fn spanning(&self, u: Set) -> Option<Vec<Vec<(usize, f64)>>> {
        let first = members(u).next()?;
        let mut tree = vec![Vec::new(); self.graph.n];
        let mut reached = bit(first);
        while reached != u {
            let edge = members(reached)
                .flat_map(|a| members(self.graph.adjacent(a) & u & !reached).map(move |b| (a, b)))
                .map(|(a, b)| (a, b, self.fraction(a, b)))
                .min_by(|x, y| x.2.total_cmp(&y.2))?;
            tree.get_mut(edge.0)?.push((edge.1, edge.2));
            tree.get_mut(edge.1)?.push((edge.0, edge.2));
            reached |= bit(edge.1);
        }
        Some(tree)
    }

    /// The IKKBZ order of the relations of `u`.
    fn ikkbz(&self, u: Set) -> Option<Vec<usize>> {
        let tree = self.spanning(u)?;
        members(u)
            .map(|root| {
                let chain = self.chain(&tree, (root, None), 1.0_f64, 0);
                let cost = chain.iter().skip(1).fold((0.0_f64, self.model.rows(root)), |(c, t), m| (c + t * m.c, t * m.t));
                (cost.0, chain.into_iter().flat_map(|m| m.rels).collect::<Vec<_>>())
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, order)| order)
    }

    /// The chain of the subtree of `node`, reached from `parent`, if it is
    /// not the root, by an edge
    /// of selectivity `s`: the node, then its children's chains merged by
    /// rank, the node joined with the head while that ranks lower.
    fn chain(&self, tree: &[Vec<(usize, f64)>], (node, parent): (usize, Option<usize>), s: f64, depth: usize) -> Vec<Module> {
        let t = s * self.model.rows(node);
        let mut head = Module { rels: vec![node], t, c: t };
        let mut below: Vec<Module> = Vec::new();
        if depth <= super::bits::MAX_RELATIONS {
            for &(child, fraction) in tree.get(node).into_iter().flatten().filter(|(c, _)| Some(*c) != parent) {
                below.extend(self.chain(tree, (child, Some(node)), fraction, depth.saturating_add(1)));
            }
        }
        // Each child's chain ascends by rank, so all of them sorted is their merge.
        below.sort_by(|a, b| a.rank().total_cmp(&b.rank()));
        let mut rest = below.into_iter().peekable();
        while parent.is_some() && rest.peek().is_some_and(|next| next.rank() < head.rank()) {
            if let Some(next) = rest.next() {
                head = head.then(next);
            }
        }
        std::iter::once(head).chain(rest).collect()
    }
}
