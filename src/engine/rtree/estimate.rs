//! Bounded cardinality estimates for broad spatial probes. Sample root
//! branches across their full range, then choose one child per level.
//! Weight sampled leaf matches by the fanout of each branch. These statistics
//! choose a plan; they never prune rows or certify a query result.

use super::node::{Entry, Node};
use super::{RTree, Visit};
use crate::disk::pager::PageId;
use crate::spatial::Bounds;

/// Planning samples a fixed number of paths regardless of result size.
/// The page budget permits ordinary trees up to four levels; deeper or
/// concurrently split paths can decline without lengthening planning.
const SAMPLES: usize = 64;
const PAGES: usize = 256;

impl RTree {
    pub(crate) fn estimated_matches(&self, bounds: Bounds) -> Option<usize> {
        let _pin = self.pin();
        let visit = self.root().ok()?;
        let mut sample = Sampling { tree: self, bounds, pages: PAGES };
        let root = sample.node(visit)?;
        if root.level == 0 {
            return Some(sample.matches(&root));
        }
        if root.entries.is_empty() {
            return Some(0);
        }
        let mut total = 0_u128;
        for ordinal in 0..SAMPLES {
            // Evenly spread starting branches avoid sampling just one
            // spatial corner when a broad query exceeds the exact count.
            let at = ordinal.checked_mul(root.entries.len())?.checked_div(SAMPLES)?;
            let entry = *root.entries.get(at)?;
            let weight = u128::try_from(root.entries.len()).ok()?;
            total = total.checked_add(sample.path(entry, root.level, weight, u64::try_from(ordinal).ok()?)?)?;
        }
        usize::try_from(total.checked_div(u128::try_from(SAMPLES).ok()?)?).ok()
    }
}

struct Sampling<'a> {
    tree: &'a RTree,
    bounds: Bounds,
    pages: usize,
}

impl Sampling<'_> {
    fn node(&mut self, visit: Visit) -> Option<Node> {
        self.pages = self.pages.checked_sub(1)?;
        let node = self.tree.read(visit.id).ok()?;
        // Following an unfinished split could exceed the sample budget or
        // change its sampling weight. Ordinary search handles those siblings;
        // statistics may simply decline and leave the planner its fallback.
        (node.level == visit.level && node.stamp == visit.stamp && !node.pending).then_some(node)
    }

    fn matches(&self, node: &Node) -> usize {
        node.entries.iter().filter(|entry| self.bounds.intersects(entry.bounds)).count()
    }

    fn path(&mut self, mut entry: Entry, mut level: u8, mut weight: u128, sample: u64) -> Option<u128> {
        loop {
            if !self.bounds.intersects(entry.bounds) {
                return Some(0);
            }
            let visit = Visit { id: PageId(entry.id), stamp: entry.value, level: level.checked_sub(1)? };
            let node = self.node(visit)?;
            if node.level == 0 {
                return weight.checked_mul(u128::try_from(self.matches(&node)).ok()?);
            }
            if node.entries.is_empty() {
                return Some(0);
            }
            let count = u64::try_from(node.entries.len()).ok()?;
            // XXH3-64 of the sample's number and the page's (RULES.md D460).
            let at = usize::try_from(crate::hash::xxhash::Words::new(0).pair(sample, visit.id.0).checked_rem(count)?).ok()?;
            entry = *node.entries.get(at)?;
            weight = weight.checked_mul(u128::from(count))?;
            level = node.level;
        }
    }
}

#[cfg(test)]
mod tests;
