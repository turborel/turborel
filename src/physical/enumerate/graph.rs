//! \[DESIGN\] The query hypergraph (RULES.md D300, D301): a node for each
//! relation, and an edge, simple or a hyperedge, for each conjunct of an
//! inner join and for each other operator, as Moerkotte and Neumann,
//! "Dynamic Programming Strikes Back", 2008, §5.8, make one of an
//! operator's total eligibility set: its left side the tables of the
//! operator's left operand its predicate reads, its right side those of
//! its right operand. A product with no conjunct is an edge between a
//! table of each operand, so the graph is connected and the product's
//! place is still searched; a non-inner operator whose predicate reads
//! no table of an operand takes that operand whole.
//!
//! Each operator keeps the conflict rules CD-C gives it
//! (`enumerate/conflicts.rs`), checked as a join is made of two sets.

use super::bits::{Set, bit, lowest_bit, within};
use super::conflicts::{self, Kind, Rule, Shape};

/// An operator of the initial tree: its shape, and its conflict rules.
#[derive(Debug, Clone)]
pub(super) struct Op {
    pub(super) shape: Shape,
    pub(super) rules: Vec<Rule>,
}

/// An edge: the sets its two sides are, the operator it is of, and the
/// conjunct of an inner join it is, if one.
#[derive(Debug, Clone, Copy)]
pub(super) struct Edge {
    pub(super) left: Set,
    pub(super) right: Set,
    pub(super) op: usize,
    pub(super) conjunct: Option<usize>,
}

impl Edge {
    /// Whether it joins `a` to `b`, one side within each.
    fn joins(&self, a: Set, b: Set) -> bool {
        (within(self.left, a) && within(self.right, b)) || (within(self.left, b) && within(self.right, a))
    }

    /// Whether it is simple: one relation on each side.
    fn simple(&self) -> bool {
        self.left.is_power_of_two() && self.right.is_power_of_two()
    }
}

/// The hypergraph of a chain of joins.
#[derive(Debug, Clone)]
pub(super) struct Graph {
    /// Its relations.
    pub(super) n: usize,
    /// The operators of the initial tree.
    pub(super) ops: Vec<Op>,
    /// Its edges.
    pub(super) edges: Vec<Edge>,
    /// Of each relation, the relations a simple edge joins it to.
    simple: Vec<Set>,
    /// The places of its hyperedges among `edges`.
    hyper: Vec<usize>,
}

impl Graph {
    /// The graph of `n` relations, the operators `shapes` of the initial
    /// tree, and of each inner one the sets each of its conjuncts reads,
    /// `conjuncts`, by the operator they belong to.
    pub(super) fn new(n: usize, shapes: &[Shape], conjuncts: &[(usize, Set)]) -> Graph {
        let ops: Vec<Op> = shapes.iter().enumerate().map(|(b, &shape)| Op { shape, rules: conflicts::rules(shapes, b) }).collect();
        let mut edges = Vec::new();
        for (o, op) in ops.iter().enumerate() {
            let s = op.shape;
            if s.kind != Kind::Inner {
                let side = |of: Set| if of & s.reads == 0 { of } else { of & s.reads };
                edges.push(Edge { left: side(s.left), right: side(s.right), op: o, conjunct: None });
                continue;
            }
            let mut any = false;
            for (c, &(_, reads)) in conjuncts.iter().enumerate().filter(|(_, (of, _))| *of == o) {
                any = true;
                edges.push(Edge { left: reads & s.left, right: reads & s.right, op: o, conjunct: Some(c) });
            }
            if !any {
                edges.push(Edge { left: lowest_bit(s.left), right: lowest_bit(s.right), op: o, conjunct: None });
            }
        }
        let mut simple = vec![0; n];
        let mut hyper = Vec::new();
        for (e, edge) in edges.iter().enumerate() {
            if edge.simple() {
                link(&mut simple, (edge.left, edge.right));
                link(&mut simple, (edge.right, edge.left));
            } else {
                hyper.push(e);
            }
        }
        Graph { n, ops, edges, simple, hyper }
    }

    /// \[DESIGN\] (RULES.md D387) Adds a simple edge of each of `pairs`,
    /// two relations and the operator of a key of their class, which the
    /// keys of the inner joins make equal though no conjunct reads both:
    /// their join judges the key the class implies.
    pub(super) fn implied(&mut self, pairs: &[(usize, Set)]) {
        for &(op, pair) in pairs {
            let (left, right) = (lowest_bit(pair), pair & !lowest_bit(pair));
            if self.edges.iter().any(|e| e.joins(left, right)) {
                continue;
            }
            self.edges.push(Edge { left, right, op, conjunct: None });
            link(&mut self.simple, (left, right));
            link(&mut self.simple, (right, left));
        }
    }

    /// All its relations.
    pub(super) fn all(&self) -> Set {
        self.n.checked_sub(1).map_or(0, super::bits::through)
    }

    /// Whether it has a hyperedge or an operator other than the inner
    /// join: DPccp's graphs have neither, DPhyp's may.
    pub(super) fn is_hyper(&self) -> bool {
        !self.hyper.is_empty() || self.ops.iter().any(|o| o.shape.kind != Kind::Inner)
    }

    /// N(S, X) (DPhyp §3.2): the relations not in `s` nor `x` a simple
    /// edge joins to `s`, and the first relation of each hyperedge's side
    /// that has none of them whose other side is within `s`.
    pub(super) fn neighbours(&self, s: Set, x: Set) -> Set {
        let taken = s | x;
        let mut out: Set = super::bits::members(s).filter_map(|r| self.simple.get(r)).fold(0, |n, &adjacent| n | adjacent) & !taken;
        for edge in self.hyper.iter().filter_map(|&e| self.edges.get(e)) {
            if within(edge.left, s) && edge.right & taken == 0 {
                out |= lowest_bit(edge.right);
            } else if within(edge.right, s) && edge.left & taken == 0 {
                out |= lowest_bit(edge.left);
            }
        }
        out
    }

    /// Whether an edge joins `a` to `b`.
    pub(super) fn connected(&self, a: Set, b: Set) -> bool {
        self.edges.iter().any(|e| e.joins(a, b))
    }

    /// The relations a simple edge joins to `r`.
    pub(super) fn adjacent(&self, r: usize) -> Set {
        self.simple.get(r).copied().unwrap_or(0) & !bit(r)
    }
}

/// Adds `to` to the simple neighbours of the relation of `from`.
fn link(simple: &mut [Set], (from, to): (Set, Set)) {
    if let Some(at) = super::bits::lowest(from).and_then(|r| simple.get_mut(r)) {
        *at |= to;
    }
}
