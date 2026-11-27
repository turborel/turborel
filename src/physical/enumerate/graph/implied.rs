//! \[DESIGN\] The edges a class of equalities implies (RULES.md D1204).
//! The equalities of a chain of inner joins make classes of columns equal
//! in every row they keep (D302, D387). A relation whose column of a class
//! is a key of it matches at most one of its rows to each row of another
//! relation of the class, so joining the two by the implied equality
//! never gives more rows than the other has, and where the keyed relation
//! is filtered it gives fewer. The graph takes an edge from such a
//! relation to each other relation of its class, a star, so the
//! enumeration may make that join before the relations between them.
//!
//! Edges add connected subgraphs, and a class every relation of which is
//! keyed would make a clique, whose pairs grow as three to the power of
//! its relations where a chain's grow as their cube. So a star is taken
//! only while the work exact dynamic programming does of the graph with
//! it, the connected subgraphs it meets and the pairs it joins, stays
//! within PLANNER.DP_BUDGET. The stars are offered from the relation of
//! fewest rows, whose join reduces most, and one that passes the budget is
//! left out, the graph as it was before it. A graph that passes the budget
//! with no star takes none and is planned as it was.

use super::Graph;
use crate::physical::enumerate::bits::{MAX_RELATIONS, Set, bit, lowest, members, members_down, subsets, through};

/// A count of work, stopped past its budget.
struct Work {
    done: usize,
    budget: usize,
}

impl Work {
    /// One more.
    fn count(&mut self) {
        self.done = self.done.saturating_add(1);
    }

    /// Whether the budget is passed.
    fn over(&self) -> bool {
        self.done > self.budget
    }
}

impl Graph {
    /// Takes each star of `stars`, a relation and the others of its class,
    /// in their order, that leaves the graph's work within `budget`.
    pub(in crate::physical::enumerate) fn implying(mut self, stars: &[(usize, Set)], budget: usize) -> Graph {
        if stars.is_empty() || self.work(budget) > budget {
            return self;
        }
        for &(hub, others) in stars {
            let before = (self.simple.clone(), self.implied.clone());
            for other in members(others & self.all() & !bit(hub)) {
                self.imply(hub, other);
                self.imply(other, hub);
            }
            if self.work(budget) > budget {
                (self.simple, self.implied) = before;
            }
        }
        self
    }

    /// Joins `from` to `to` by an implied edge.
    fn imply(&mut self, from: usize, to: usize) {
        for adjacency in [&mut self.simple, &mut self.implied] {
            if let Some(at) = adjacency.get_mut(from) {
                *at |= bit(to);
            }
        }
    }

    /// Whether an implied edge joins a relation of `a` to one of `b`.
    pub(super) fn implies(&self, a: Set, b: Set) -> bool {
        members(a).filter_map(|r| self.implied.get(r)).any(|&to| to & b != 0)
    }

    /// The work exact dynamic programming does of the graph: each
    /// connected subgraph DPhyp's traversal meets and each complement it
    /// offers one (`enumerate/dphyp.rs`), counted until past `budget`. Of
    /// a graph of simple edges these are DPccp's connected subgraphs and
    /// its csg-cmp pairs.
    pub(in crate::physical::enumerate) fn work(&self, budget: usize) -> usize {
        let mut work = Work { done: 0, budget };
        let all = self.all();
        for v in members_down(all) {
            self.emitted(bit(v), &mut work);
            self.grown(bit(v), through(v) | !all, &mut work, 0);
            if work.over() {
                break;
            }
        }
        work.done
    }

    /// Counts the subgraph `s1` and each of its complements.
    fn emitted(&self, s1: Set, work: &mut Work) {
        work.count();
        let x = s1 | lowest(s1).map_or(0, through) | !self.all();
        let n = self.neighbours(s1, x);
        for v in members_down(n) {
            if work.over() {
                return;
            }
            work.count();
            self.complements(bit(v), x | (through(v) & n), work, 0);
        }
    }

    /// Counts `s1` grown by each subset of its neighbourhood outside `x`,
    /// each with its complements, then grown further.
    fn grown(&self, s1: Set, x: Set, work: &mut Work, depth: usize) {
        if depth > MAX_RELATIONS {
            return;
        }
        let n = self.neighbours(s1, x);
        for sub in subsets(n) {
            if work.over() {
                return;
            }
            self.emitted(s1 | sub, work);
        }
        for sub in subsets(n) {
            if work.over() {
                return;
            }
            self.grown(s1 | sub, x | n, work, depth.saturating_add(1));
        }
    }

    /// Counts the complement `s2` grown by each subset of its
    /// neighbourhood outside `x`, then grown further.
    fn complements(&self, s2: Set, x: Set, work: &mut Work, depth: usize) {
        if depth > MAX_RELATIONS {
            return;
        }
        let n = self.neighbours(s2, x);
        for sub in subsets(n) {
            if work.over() {
                return;
            }
            work.count();
            self.complements(s2 | sub, x | n, work, depth.saturating_add(1));
        }
    }
}
