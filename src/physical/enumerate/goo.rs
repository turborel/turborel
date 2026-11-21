//! \[DESIGN\] Greedy Operator Ordering guiding dynamic programming
//! (Neumann and Radke, "Adaptive Optimization of Very Large Join
//! Queries", 2018, §4.3, Fig. 7; RULES.md D304): GOO (Fegaras 1998, as
//! the paper gives it) joins, of the plans it holds, the two that may be
//! joined giving fewest rows, until one is left, a bushy tree. Then, while
//! the budget lasts, the costliest subtree of at most `k` relations whose
//! parent has more is planned again by dynamic programming, as a
//! relation once done, and the joins above it again, so a relation may
//! move as far as `k` places from where the greedy step put it.

use super::bits::{Set, bit, count, members, within};
use super::space::Space;

/// A node of the greedy tree: its set, and its operands' nodes if a join.
#[derive(Debug, Clone, Copy)]
struct Node {
    set: Set,
    operands: Option<(usize, usize)>,
}

/// Which dynamic programming plans a subtree again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Inner {
    /// Linearized DP, of graphs with simple edges alone.
    Linearized,
    /// DPhyp, of any.
    Hyper,
}

impl Space<'_> {
    /// GOO-DP of the relations of `u`, subtrees of at most `k` planned
    /// again by `inner` until `budget` table entries are made; whether a
    /// plan of all of them was found.
    pub(super) fn goo_dp(&mut self, u: Set, (k, budget): (usize, usize), inner: Inner) -> bool {
        let Some(nodes) = self.goo(u) else { return false };
        let mut left = budget;
        let mut done: Vec<usize> = Vec::new();
        while left > 0 {
            let Some(at) = self.costliest(&nodes, k, &done) else { break };
            let Some(node) = nodes.get(at) else { break };
            let before = self.table.len();
            if inner == Inner::Hyper || !self.linearized(node.set) {
                self.dphyp(node.set);
            }
            left = left.saturating_sub(self.table.len().saturating_sub(before).max(1));
            done.push(at);
        }
        // The nodes were made bottom-up, so each is joined again after its operands.
        for node in &nodes {
            if let Some((a, b)) = node.operands
                && let (Some(l), Some(r)) = (nodes.get(a), nodes.get(b))
            {
                self.join(l.set, r.set);
            }
        }
        self.has(u)
    }

    /// GOO: the greedy tree of the relations of `u`, its nodes bottom-up,
    /// the root last; none if two plans could not be joined.
    fn goo(&mut self, u: Set) -> Option<Vec<Node>> {
        let mut nodes: Vec<Node> = members(u).map(|r| Node { set: bit(r), operands: None }).collect();
        let mut forest: Vec<usize> = (0..nodes.len()).collect();
        while forest.len() > 1 {
            let (first, second) = self.smallest_join(&nodes, &forest)?;
            let (left, right) = (*forest.get(first)?, *forest.get(second)?);
            let (left_set, right_set) = (nodes.get(left)?.set, nodes.get(right)?.set);
            if !self.join(left_set, right_set) {
                return None;
            }
            nodes.push(Node { set: left_set | right_set, operands: Some((left, right)) });
            forest.retain(|&tree| tree != left && tree != right);
            forest.push(nodes.len().saturating_sub(1));
        }
        Some(nodes)
    }

    /// Of the trees `forest` of `nodes`, the places of the two that may
    /// be joined giving fewest rows.
    fn smallest_join(&self, nodes: &[Node], forest: &[usize]) -> Option<(usize, usize)> {
        let set = |i: usize| forest.get(i).and_then(|&f| nodes.get(f)).map_or(0, |n| n.set);
        let pairs = (0..forest.len()).flat_map(|i| (i.saturating_add(1)..forest.len()).map(move |j| (i, j)));
        pairs
            .filter_map(|(i, j)| {
                let applied = self.applies(set(i), set(j))?;
                Some(((i, j), self.estimate_of(&applied)?))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(pair, _)| pair)
    }

    /// The costliest node of at most `k` relations whose parent has more,
    /// or which is the root, not among `done`.
    fn costliest(&self, nodes: &[Node], k: usize, done: &[usize]) -> Option<usize> {
        let parent = |at: usize| nodes.iter().find(|n| n.operands.is_some_and(|(a, b)| a == at || b == at));
        let cost = |set: Set| self.table.get(&set).and_then(super::space::Entry::best).map_or(0.0_f64, |c| c.cost);
        (0..nodes.len())
            .filter(|at| !done.contains(at))
            .filter_map(|at| Some((at, nodes.get(at)?)))
            .filter(|(at, n)| count(n.set) > 1 && count(n.set) <= k && parent(*at).is_none_or(|p| count(p.set) > k))
            .filter(|(_, n)| !done.iter().filter_map(|&d| nodes.get(d)).any(|d| within(n.set, d.set)))
            .max_by(|a, b| cost(a.1.set).total_cmp(&cost(b.1.set)))
            .map(|(at, _)| at)
    }
}
