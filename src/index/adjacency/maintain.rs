//! \[DESIGN\] An adjacency index's maintenance (RULES.md D31, D35,
//! D-GRAPH-ADJACENCY): the pass over its keys an opened index's count of
//! them waits for (`index/stats.rs`), and the compacted adjacency built
//! again of the tree (`csr.rs`): when there is none, or the delta marks
//! more vertices than `LEAST_MARKS` and an eighth of those it numbers. A
//! build is made while writers go on (`delta.rs`). One that does not fit
//! the budget leaves the tree alone read, and is tried again only once
//! the index holds twice the entries, so maintenance does not read the
//! index whole again and again for nothing.

use super::AdjacencyIndex;
use super::csr::Builder;
use super::delta::LEAST_MARKS;
use super::keys::{parse, tag};
use crate::engine::tree::Direction as Order;
use crate::error::Result;
use crate::graph::adjacency::Direction;
use crate::index::stats::Keys;
use crate::sync::lock_builtin;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// \[DESIGN\] The share of its vertices the delta may mark before the
/// compacted adjacency is built again: an eighth, so a build's cost, a
/// pass over the index, is spread over writes to as many vertices.
const MARKED_SHARE: usize = 8;

impl AdjacencyIndex {
    /// A builder of a compacted adjacency within its budget and
    /// ADJACENCY.CSR_BYTES, if it has room.
    pub(super) fn builder(&self) -> Option<Builder> {
        let most = self.most.load(Ordering::Acquire);
        let budget = self.budget.as_ref().filter(|_| most > 0)?;
        Builder::new(budget, most).ok()
    }

    /// Notes that a build did not fit: the tree is read alone until the
    /// index holds twice its entries.
    pub(super) fn refuse(&self) {
        // A poisoned lock leaves the adjacency, which stays exact.
        let _ = self.delta.drop_csr();
        *lock_builtin(&self.refused) = Some(self.keys.live());
    }

    /// Whether a build of the compacted adjacency is due.
    pub(super) fn compaction_due(&self) -> bool {
        if self.budget.is_none() || self.most.load(Ordering::Acquire) == 0 {
            return false;
        }
        match self.delta.current() {
            Ok(Some(csr)) => self.delta.marked() > LEAST_MARKS.max(csr.vertices().checked_div(MARKED_SHARE).unwrap_or_default()),
            Ok(None) => lock_builtin(&self.refused).is_none_or(|n| {
                self.keys.live() >= n.saturating_mul(2).max(n.saturating_add(u64::try_from(LEAST_MARKS).unwrap_or(u64::MAX)))
            }),
            Err(_) => false,
        }
    }

    /// `maintain`: the pass over its keys, if due, and a build, if due.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -910 if a lock is poisoned.
    pub(super) fn maintained(&self) -> Result<bool> {
        let counted = self.keys.count(|| Ok(self.every_key()))?;
        let compacted = self.compaction_due() && self.compact()?;
        Ok(counted || compacted)
    }

    /// Every entry's index key, in bytes, of the first direction kept.
    fn every_key(&self) -> Keys<'_> {
        let direction = self.layout.kept.directions().first().copied().unwrap_or(Direction::Out);
        let scan = self.tree.scan(Bound::Included(&[tag(direction)]), Bound::Excluded(&[tag(direction).saturating_add(1)]), Order::Forward);
        Box::new(scan.map(move |entry| {
            let (key, _) = entry?;
            let parsed = parse(&key, &self.layout)?;
            Ok(match direction {
                Direction::Out => [parsed.vertex, parsed.other, parsed.included].concat(),
                Direction::In => [parsed.other, parsed.vertex, parsed.included].concat(),
            })
        }))
    }

    /// Builds the compacted adjacency of the tree, while writers go on:
    /// whether it did, not if another build is under way or it did not fit.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -910 if a lock is poisoned.
    pub(super) fn compact(&self) -> Result<bool> {
        let Ok(_turn) = self.building.try_lock() else { return Ok(false) };
        let Some(mut builder) = self.builder() else {
            self.refuse();
            return Ok(false);
        };
        self.delta.begin()?;
        match self.read_into(&mut builder) {
            Ok(true) => {
                self.delta.install(&Arc::new(builder.finish()))?;
                *lock_builtin(&self.refused) = None;
                Ok(true)
            }
            Ok(false) => {
                self.delta.abandon()?;
                self.refuse();
                Ok(false)
            }
            Err(e) => {
                // The build's error is the one told; its marks are dropped either way.
                let _ = self.delta.abandon();
                Err(e)
            }
        }
    }

    /// Gives `builder` every entry, forward then reverse, in the tree's
    /// order: whether they all fit.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if an entry is not of the layout.
    fn read_into(&self, builder: &mut Builder) -> Result<bool> {
        for &direction in self.layout.kept.directions() {
            let (low, high) = ([tag(direction)], [tag(direction).saturating_add(1)]);
            for entry in self.tree.scan(Bound::Included(&low), Bound::Excluded(&high), Order::Forward) {
                let (key, _) = entry?;
                if !builder.push(&parse(&key, &self.layout)?) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}
