//! \[DESIGN\] RULES.md D364: a tree of losers (Knuth, The Art of Computer
//! Programming, vol. 3, §5.4.1; Graefe, "Implementing Sorting in Database
//! Systems", 2006), which merges k sources with about log k comparisons
//! for each row given and no sift of a heap. Each inner node keeps the
//! source that lost the match played there; the winner of the whole tree
//! is kept apart. When the winner's source moves to its next row, only
//! the matches on its path to the root are played again. The comparison
//! is the caller's. It must be a total order, so a tie must be broken,
//! which a merge breaks by the sources' order, keeping it stable.

/// The children of an inner node.
const FANOUT: usize = 2;

/// A tree of losers over the sources `0..k`.
pub(super) struct Losers {
    /// The loser of the match at each inner node, from 1; node 0 unused.
    tree: Vec<usize>,
    /// The source whose row goes first.
    winner: usize,
}

impl Losers {
    /// The tree of `k` sources, `before(a, b)` saying whether source `a`'s
    /// next row goes before source `b`'s.
    pub(super) fn new(k: usize, mut before: impl FnMut(usize, usize) -> bool) -> Losers {
        let mut tree = vec![0_usize; k.max(1)];
        // The winner at each node, inner nodes from 1, leaves from `k`.
        let mut won: Vec<usize> = vec![0_usize; k];
        won.extend(0..k);
        for node in (1..k).rev() {
            let left = won.get(node.saturating_mul(FANOUT)).copied().unwrap_or(0);
            let right = won.get(node.saturating_mul(FANOUT).saturating_add(1)).copied().unwrap_or(0);
            let (winner, loser) = if before(right, left) { (right, left) } else { (left, right) };
            if let Some(slot) = tree.get_mut(node) {
                *slot = loser;
            }
            if let Some(slot) = won.get_mut(node) {
                *slot = winner;
            }
        }
        let winner = if k > 1 { won.get(1).copied().unwrap_or(0) } else { 0 };
        Losers { tree, winner }
    }

    /// The source whose row goes first.
    pub(super) fn winner(&self) -> usize {
        self.winner
    }

    /// Plays again the matches of the winner's source, whose next row has
    /// changed, by `before`.
    pub(super) fn replay(&mut self, mut before: impl FnMut(usize, usize) -> bool) {
        let k = self.tree.len();
        let mut winner = self.winner;
        let mut node = winner.saturating_add(k).checked_div(FANOUT).unwrap_or(0);
        while node >= 1 {
            if let Some(slot) = self.tree.get_mut(node)
                && before(*slot, winner)
            {
                std::mem::swap(slot, &mut winner);
            }
            node = node.checked_div(FANOUT).unwrap_or(0);
        }
        self.winner = winner;
    }
}

// Unit tests: the tree against a sort, ties to the earlier source
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// The values of `sources`, each ascending, merged by a tree of losers,
    /// as (value, source).
    fn merged(sources: &[Vec<u32>]) -> Vec<(u32, usize)> {
        let mut at = vec![0_usize; sources.len()];
        let head = |places: &[usize], s: usize| sources[s].get(places[s]).copied();
        let before = |places: &[usize], a: usize, b: usize| match (head(places, a), head(places, b)) {
            (Some(x), Some(y)) => (x, a) < (y, b),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => a < b,
        };
        let mut tree = Losers::new(sources.len(), |a, b| before(&at, a, b));
        let mut out = Vec::new();
        while let Some(v) = head(&at, tree.winner()) {
            out.push((v, tree.winner()));
            at[tree.winner()] = at[tree.winner()].saturating_add(1);
            tree.replay(|a, b| before(&at, a, b));
        }
        out
    }

    /// Verifies: [DESIGN] RULES.md D364: a tree of losers of 1 to 9 sources,
    /// of equal values among them and of sources empty, merges them as a
    /// stable sort of every value by its source would, each tie going to
    /// the earlier source.
    #[test]
    fn losers_merge_as_a_stable_sort() {
        for k in 1_usize..=9 {
            let sources: Vec<Vec<u32>> = (0..k)
                .map(|s| (0..(s * 7 % 5)).map(|n| u32::try_from((n * 3 + s) % 4).unwrap()).collect::<Vec<u32>>())
                .map(|mut v| {
                    v.sort_unstable();
                    v
                })
                .collect();
            let mut expected: Vec<(u32, usize)> = sources.iter().enumerate().flat_map(|(s, v)| v.iter().map(move |&x| (x, s))).collect();
            expected.sort_unstable();
            assert_eq!(merged(&sources), expected, "{k} sources");
        }
    }
}
