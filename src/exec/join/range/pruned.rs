//! \[DESIGN\] The comparison a range join answers beside its bounds
//! (`physical::Pruned`, RULES.md D580): over the sorted rows, a complete
//! binary tree whose every node holds the greatest of its rows' values,
//! of a comparison by `>` or `>=`, or the least, by `<` or `<=`. A node
//! whose value does not hold has no row whose value does, and is passed
//! over, so the rows between a left row's bounds whose value holds are
//! found in a time of the rows found and the tree's height, as of an
//! interval overlap each searching row finds the rows that begin before it
//! ends, then of them those that end after it begins.

use std::cmp::Ordering;

use crate::value::{CompOp, Value, compare};

/// \[DESIGN\] RULES.md R-CF-6: the most nodes waiting to be looked at,
/// two for each level of a tree of as many rows as memory can hold.
const MOST_WAITING: usize = 128;

/// The tree over the values of the sorted rows, in their order.
pub(super) struct Tree {
    /// The leaves' first node: a power of two.
    leaves: usize,
    /// Of each node from 1, the best of its rows' values, none past the
    /// last row.
    best: Vec<Option<Value>>,
    /// How a row's value compares with a left row's.
    op: CompOp,
}

impl Tree {
    /// The tree of `values`, compared by `op`; none if two values are not
    /// comparable.
    pub(super) fn of(values: Vec<Value>, op: CompOp) -> Option<Tree> {
        let leaves = values.len().max(1).checked_next_power_of_two()?;
        let mut best: Vec<Option<Value>> = vec![None; leaves];
        best.extend(values.into_iter().map(Some));
        best.resize(leaves.checked_mul(2)?, None);
        let greatest = matches!(op, CompOp::Gt | CompOp::Ge);
        for node in (1..leaves).rev() {
            let left = best.get(node.checked_mul(2)?)?.as_ref();
            let right = best.get(node.checked_mul(2)?.checked_add(1)?)?.as_ref();
            let chosen = match (left, right) {
                (Some(a), Some(b)) => {
                    let after = compare(a, b)? == Ordering::Less;
                    if after == greatest { b.clone() } else { a.clone() }
                }
                (Some(only), None) | (None, Some(only)) => only.clone(),
                (None, None) => continue,
            };
            *best.get_mut(node)? = Some(chosen);
        }
        Some(Tree { leaves, best, op })
    }

    /// Adds to `out` the places in `range` of the rows whose value holds
    /// against `probe`; none if one is not comparable with it, and then
    /// what was added is not all.
    pub(super) fn found(&self, range: std::ops::Range<usize>, probe: &Value, out: &mut Vec<usize>) -> Option<()> {
        let mut waiting: Vec<(usize, usize, usize)> = vec![(1, 0, self.leaves)];
        while let Some((node, start, end)) = waiting.pop() {
            if end <= range.start || start >= range.end {
                continue;
            }
            let Some(value) = self.best.get(node)?.as_ref() else { continue };
            if !self.op.holds(compare(value, probe)?) {
                continue;
            }
            if node >= self.leaves {
                out.push(node.checked_sub(self.leaves)?);
                continue;
            }
            if waiting.len() >= MOST_WAITING {
                return None;
            }
            let middle = start.saturating_add(end.saturating_sub(start) >> 1);
            let child = node.checked_mul(2)?;
            waiting.push((child.checked_add(1)?, middle, end));
            waiting.push((child, start, middle));
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn exact(n: i64) -> Value {
        Value::Exact(Decimal::new(n.into(), 0))
    }

    /// Verifies: [DESIGN] RULES.md D580: the rows in a range whose value
    /// holds are found, by the greatest of `>` and `>=` and the least of
    /// `<` and `<=`, at a boundary equal or not.
    #[test]
    fn finds_the_rows_whose_value_holds() {
        let values: Vec<i64> = vec![7, 2, 9, 4, 4, 1, 8];
        let tree = |op| Tree::of(values.iter().map(|&v| exact(v)).collect(), op).unwrap();
        let found = |t: &Tree, range: std::ops::Range<usize>, probe: i64| {
            let mut out = Vec::new();
            t.found(range, &exact(probe), &mut out).unwrap();
            out.sort_unstable();
            out
        };
        assert_eq!(found(&tree(CompOp::Gt), 0..7, 4), vec![0, 2, 6]);
        assert_eq!(found(&tree(CompOp::Ge), 0..7, 4), vec![0, 2, 3, 4, 6]);
        assert_eq!(found(&tree(CompOp::Ge), 1..5, 4), vec![2, 3, 4]);
        assert_eq!(found(&tree(CompOp::Lt), 0..7, 4), vec![1, 5]);
        assert_eq!(found(&tree(CompOp::Le), 2..7, 4), vec![3, 4, 5]);
        assert_eq!(found(&tree(CompOp::Gt), 0..7, 9), Vec::<usize>::new());
        assert_eq!(found(&tree(CompOp::Gt), 3..3, 0), Vec::<usize>::new());
    }

    /// Verifies: [DESIGN] RULES.md D580: values not comparable make no
    /// tree, and a left row's value not comparable finds nothing known.
    #[test]
    fn values_not_comparable_are_found_out() {
        let mixed = vec![exact(1), Value::Char("a".into())];
        assert!(Tree::of(mixed, CompOp::Gt).is_none());
        let tree = Tree::of(vec![exact(1), exact(2)], CompOp::Gt).unwrap();
        assert!(tree.found(0..2, &Value::Char("a".into()), &mut Vec::new()).is_none());
        assert!(Tree::of(Vec::new(), CompOp::Lt).is_some());
    }
}
