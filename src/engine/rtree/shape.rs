//! \[DESIGN\] The shape of an R-link tree, level by level: nodes, bytes,
//! average fill and the total overlap of node envelopes, which together
//! show what packing, splits and reclamation leave behind (RULES.md D139).
//! Measurement only; nothing here changes the tree or guides a search.

use super::node::{ENTRY_BYTES, HEADER};
use super::scan::Walk;
use super::{RTree, corrupt};
use crate::error::Result;
use crate::spatial::Bounds;

/// One level of a tree, leaves at level 0.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Level {
    /// The level, counted up from the leaves.
    pub level: u8,
    /// Its nodes.
    pub nodes: u64,
    /// The bytes of the pages its nodes take.
    pub bytes: u64,
    /// The bytes of those pages its headers and entries use.
    pub used: u64,
    /// Its entries: references at a leaf, children above.
    pub entries: u64,
    /// Entries per node over a node's capacity, from 0 to 1.
    pub fill: f64,
    /// The summed area where two of its nodes' envelopes intersect, each
    /// pair once.
    pub overlap: f64,
}

impl RTree {
    /// Each level's shape, leaves first, from a walk of every node.
    ///
    /// # Errors
    ///
    /// Storage errors or malformed links.
    pub fn shape(&self) -> Result<Vec<Level>> {
        let page = u64::try_from(self.inner.pool.capacity(self.inner.file)?).unwrap_or(u64::MAX);
        let mut levels: Vec<(Level, Vec<Bounds>)> = Vec::new();
        let mut walk = Walk::new(self.clone(), None);
        while let Some((_, node)) = walk.node()? {
            let at = usize::from(node.level);
            while levels.len() <= at {
                let level = u8::try_from(levels.len()).map_err(|_| corrupt("its height does not fit"))?;
                levels.push((Level { level, ..Level::default() }, Vec::new()));
            }
            let (level, envelopes) = levels.get_mut(at).ok_or_else(|| corrupt("a level is missing"))?;
            let entries = u64::try_from(node.entries.len()).unwrap_or(u64::MAX);
            let bytes = HEADER.saturating_add(node.entries.len().saturating_mul(ENTRY_BYTES));
            level.nodes = level.nodes.saturating_add(1);
            level.entries = level.entries.saturating_add(entries);
            level.used = level.used.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
            level.bytes = level.bytes.saturating_add(page);
            envelopes.extend(node.bounds());
        }
        let capacity = f64::from(u32::try_from(self.inner.capacity).unwrap_or(u32::MAX));
        Ok(levels
            .into_iter()
            .map(|(mut level, envelopes)| {
                level.fill = ratio(level.entries, level.nodes) / capacity;
                level.overlap = overlap(envelopes);
                level
            })
            .collect())
    }
}

fn ratio(a: u64, b: u64) -> f64 {
    let (a, b) = (u32::try_from(a).map_or(f64::MAX, f64::from), u32::try_from(b).map_or(f64::MAX, f64::from));
    if b > 0.0_f64 { a / b } else { 0.0_f64 }
}

/// Pairwise intersection areas by a sweep along X: only envelopes whose X
/// ranges meet are compared.
fn overlap(mut envelopes: Vec<Bounds>) -> f64 {
    envelopes.sort_by(|a, b| a.lower()[0].total_cmp(&b.lower()[0]));
    let mut total = 0.0_f64;
    for (at, a) in envelopes.iter().enumerate() {
        for b in envelopes.iter().skip(at.saturating_add(1)) {
            if b.lower()[0] > a.upper()[0] {
                break;
            }
            total += a.intersection(*b).map_or(0.0_f64, area);
        }
    }
    total
}

fn area(b: Bounds) -> f64 {
    let (lower, upper) = (b.lower(), b.upper());
    (upper[0] - lower[0]) * (upper[1] - lower[1])
}
