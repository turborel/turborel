//! \[DESIGN\] (RULES.md D666) The tree a phase transfers along: of a
//! chain's leaves, joined by its hash joins' keys, a maximum spanning tree
//! rooted at the leaf of the most rows, each edge weighed by the keys its
//! two leaves share, as LargestRoot builds it (Zhao et al., Robust
//! Predicate Transfer, SIGMOD 2025, §4). For a chain acyclic once its
//! equalities are classes, as every query of the join-order workload is,
//! one pass from the leaves to the root and one back reduce each leaf to
//! the rows some row of the chain holds. A key no edge of the tree is,
//! of a cycle, transfers nothing; its join still filters as any.
//!
//! The steps are in order: from each leaf to its parent, a leaf's after
//! those of the leaves below it; then from each parent to its leaves, a
//! leaf's before those below it.

use super::Step;

/// The steps along the tree of `rows.len()` leaves of `rows` estimated
/// rows each, joined by `keys`, in order: up, then down. None of a chain
/// whose keys do not join every leaf.
pub(super) fn schedule(rows: &[f64], keys: &[Step]) -> Option<(Vec<Step>, Vec<Step>)> {
    let root = rows.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(n, _)| n)?;
    let mut held = vec![false; rows.len()];
    *held.get_mut(root)? = true;
    let mut down: Vec<Step> = Vec::with_capacity(rows.len());
    for _ in 1..rows.len() {
        let step = heaviest(&held, keys)?;
        *held.get_mut(step.to.0)? = true;
        down.push(step);
    }
    let up = down.iter().rev().map(|s| Step { from: s.to, to: s.from, nulls: s.nulls }).collect();
    Some((up, down))
}

/// Of the keys from a leaf of the tree, `held`, to one not, the first of
/// the pair of leaves most keys join, as a step from the tree's leaf.
fn heaviest(held: &[bool], keys: &[Step]) -> Option<Step> {
    let crossing = |s: &Step| {
        let (from, to) = (in_tree(held, s.from.0), in_tree(held, s.to.0));
        match (from, to) {
            (true, false) => Some(*s),
            (false, true) => Some(Step { from: s.to, to: s.from, nulls: s.nulls }),
            (true, true) | (false, false) => None,
        }
    };
    let mut best: Option<(usize, Step)> = None;
    for s in keys.iter().filter_map(crossing) {
        let weight = keys.iter().filter(|k| joins(k, (s.from.0, s.to.0))).count();
        if best.is_none_or(|(w, _)| weight > w) {
            best = Some((weight, s));
        }
    }
    best.map(|(_, s)| s)
}

/// Whether leaf `leaf` is in the tree.
fn in_tree(held: &[bool], leaf: usize) -> bool {
    held.get(leaf).copied().unwrap_or(false)
}

/// Whether the key `k` joins the leaves `a` and `b`, either way.
fn joins(k: &Step, (a, b): (usize, usize)) -> bool {
    (k.from.0 == a && k.to.0 == b) || (k.from.0 == b && k.to.0 == a)
}

// Unit tests: the tree and its order (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// A key of column 0 of leaf `a` and column 1 of leaf `b`.
    fn key(a: usize, b: usize) -> Step {
        Step { from: (a, 0), to: (b, 1), nulls: false }
    }

    /// Verifies: [DESIGN] RULES.md D666: a star's root is its largest
    /// leaf; each other leaf sends up before the root sends down, and the
    /// root sends to each.
    #[test]
    fn a_star_is_rooted_at_its_largest_leaf() {
        let (up, down) = schedule(&[10.0_f64, 1000.0_f64, 5.0_f64, 7.0_f64], &[key(0, 1), key(2, 1), key(1, 3)]).unwrap();
        assert!(up.iter().all(|s| s.to.0 == 1), "every leaf sends to the root");
        assert!(down.iter().all(|s| s.from.0 == 1), "the root sends to every leaf");
        assert_eq!(down.len(), 3_usize);
        assert_eq!(up.iter().find(|s| s.from.0 == 3).unwrap().from, (3_usize, 1_usize), "the root's key turned around");
    }

    /// Verifies: [DESIGN] RULES.md D666: of a path, each leaf sends up
    /// after the leaves below it and down before them.
    #[test]
    fn a_path_goes_up_from_its_end_and_back() {
        let (up, down) = schedule(&[1.0_f64, 2.0_f64, 100.0_f64], &[key(0, 1), key(1, 2)]).unwrap();
        assert_eq!(up.iter().map(|s| (s.from.0, s.to.0)).collect::<Vec<_>>(), vec![(0_usize, 1_usize), (1, 2)]);
        assert_eq!(down.iter().map(|s| (s.from.0, s.to.0)).collect::<Vec<_>>(), vec![(2_usize, 1_usize), (1, 0)]);
    }

    /// Verifies: [DESIGN] RULES.md D666 (MC/DC): the pair of leaves most
    /// keys join is the edge taken; a chain whose keys leave a leaf alone
    /// has no tree.
    #[test]
    fn the_heaviest_pair_is_taken_and_a_lone_leaf_has_none() {
        let keys = [key(0, 2), key(1, 2), key(2, 1)];
        let (_, down) = schedule(&[1.0_f64, 1.0_f64, 9.0_f64], &keys).unwrap();
        assert_eq!(down.first().map(|s| s.to.0), Some(1_usize), "two keys join leaves 1 and 2");
        assert!(schedule(&[1.0_f64, 1.0_f64, 9.0_f64], &[key(0, 2)]).is_none());
    }
}
