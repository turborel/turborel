//! \[DESIGN\] DPhyp (Moerkotte and Neumann, "Dynamic Programming Strikes
//! Back", 2008, §3), over a set of the graph's relations (RULES.md D300):
//! the connected subgraphs are enumerated by growing each from its least
//! relation through its neighbourhood, never into relations a caller
//! higher up handles, and each one's connected complements likewise, so
//! every csg-cmp pair is met once and every set's plans are complete
//! before a larger set uses them. Of a graph with simple edges alone the
//! neighbourhood is that of DPccp (Moerkotte and Neumann, "Analysis of Two
//! Existing and One New Dynamic Programming Algorithm for the Generation
//! of Optimal Bushy Join Trees without Cross Products", 2006, §3), and the
//! enumeration is DPccp's.
//!
//! Each call grows its set by at least one relation, so the recursion is
//! never deeper than `MAX_RELATIONS` (R-CF-6), which it checks.
//!
//! `count` is the count of connected subgraphs of Neumann and Radke,
//! "Adaptive Optimization of Very Large Join Queries", 2018, Fig. 3: the
//! same traversal without a table, stopped once past a budget.

use super::bits::{MAX_RELATIONS, Set, bit, lowest, members, members_down, subsets, through};
use super::space::Space;

impl Space<'_> {
    /// Solve: the plans of every connected subset of `u`.
    pub(super) fn dphyp(&mut self, u: Set) {
        for v in members_down(u) {
            let s = bit(v);
            self.emit_csg(s, u, 0);
            self.enumerate_csg(s, through(v) | !u, (u, 0));
        }
    }

    /// EmitCsg: each connected complement of `s1` among the relations
    /// after its least, joined to it.
    fn emit_csg(&mut self, s1: Set, u: Set, depth: usize) {
        let x = s1 | lowest(s1).map_or(0, through) | !u;
        let n = self.graph.neighbours(s1, x);
        for v in members_down(n) {
            let s2 = bit(v);
            if self.graph.connected(s1, s2) {
                self.join(s1, s2);
            }
            self.enumerate_cmp(s1, s2, (x | (through(v) & n), depth));
        }
    }

    /// EnumerateCsgRec: `s1` grown by each subset of its neighbourhood
    /// outside `x`, each emitted if it has plans, then grown further.
    fn enumerate_csg(&mut self, s1: Set, x: Set, (u, depth): (Set, usize)) {
        if depth > MAX_RELATIONS {
            return;
        }
        let n = self.graph.neighbours(s1, x);
        for sub in subsets(n) {
            if self.has(s1 | sub) {
                self.emit_csg(s1 | sub, u, depth);
            }
        }
        for sub in subsets(n) {
            self.enumerate_csg(s1 | sub, x | n, (u, depth.saturating_add(1)));
        }
    }

    /// EnumerateCmpRec: the complement `s2` of `s1` grown by each subset
    /// of its neighbourhood outside `x`, each joined to `s1` if it has
    /// plans and an edge joins them, then grown further.
    fn enumerate_cmp(&mut self, s1: Set, s2: Set, (x, depth): (Set, usize)) {
        if depth > MAX_RELATIONS {
            return;
        }
        let n = self.graph.neighbours(s2, x);
        for sub in subsets(n) {
            let grown = s2 | sub;
            if self.has(grown) && self.graph.connected(s1, grown) {
                self.join(s1, grown);
            }
        }
        for sub in subsets(n) {
            self.enumerate_cmp(s1, s2 | sub, (x | n, depth.saturating_add(1)));
        }
    }

    /// countCC: the connected subsets of `u`, counted until past `budget`.
    pub(super) fn count(&self, u: Set, budget: usize) -> usize {
        let mut c = 0_usize;
        for v in members(u) {
            c = c.saturating_add(1);
            if c > budget {
                return c;
            }
            let before = through(v) & !bit(v);
            c = self.count_from(bit(v), before | !u, (c, budget), 0);
        }
        c
    }

    /// countCCRec: `s` grown by each subset of its neighbourhood outside
    /// `x`, each counted.
    fn count_from(&self, s: Set, x: Set, (mut c, budget): (usize, usize), depth: usize) -> usize {
        if depth > MAX_RELATIONS {
            return c;
        }
        let n = self.graph.neighbours(s, x);
        for sub in subsets(n) {
            c = c.saturating_add(1);
            if c > budget {
                return c;
            }
            c = self.count_from(s | sub, x | n, (c, budget), depth.saturating_add(1));
        }
        c
    }
}
