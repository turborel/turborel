//! \[DESIGN\] A geometry index's tree of envelopes, for a join that
//! descends two of them together (RULES.md D138). The engine's join
//! prunes each pair of nodes whose envelopes are apart, so every pair of
//! entries whose keyed envelopes meet is given, once while both are held,
//! with no function called under a latch. Keys are the projected
//! envelopes the index's searches give, so a reader rechecks a row's
//! version against them as it does a search's entry (`KeyProjection`).

use std::sync::Arc;

use super::{RECTANGLE_COLUMNS, probe, unsupported};
use crate::engine::rtree::RTree;
use crate::error::Result;
use crate::index::Key;
use crate::memory::MemoryBudget;
use crate::spatial::Bounds;
use crate::storage::RowId;

/// Pairs of entries of two indexes, each its key and row id, as they are
/// read.
pub type KeyPairs = Box<dyn Iterator<Item = Result<((Key, RowId), (Key, RowId))>> + Send>;

/// The envelopes of a geometry index of one SRID, as its tree holds them.
#[derive(Clone)]
pub struct Envelopes {
    tree: RTree,
    srid: i32,
}

impl Envelopes {
    pub(super) fn new(tree: RTree, srid: i32) -> Envelopes {
        Envelopes { tree, srid }
    }

    /// The SRID of its column's values.
    pub fn srid(&self) -> i32 {
        self.srid
    }

    /// The hull of its envelopes, none if it has none. Every entry whose
    /// row a reader may see lies in it.
    ///
    /// # Errors
    ///
    /// Storage errors or malformed links.
    pub fn hull(&self) -> Result<Option<Bounds>> {
        self.tree.envelope()
    }

    /// Its entries paired with `other`'s whose envelopes meet, boundaries
    /// included, each pair once while both entries are held. The pending
    /// pairs of nodes are charged to `budget`.
    ///
    /// # Errors
    ///
    /// The method's unsupported error if the two are of different SRIDs,
    /// whose envelopes are not comparable; storage errors; SQLCODE -918 past the budget.
    pub fn pairs(&self, other: &Envelopes, budget: &Arc<MemoryBudget>) -> Result<KeyPairs> {
        if self.srid != other.srid {
            return Err(unsupported("cannot pair envelopes of different SRIDs"));
        }
        let joined = self.tree.join(&other.tree, budget)?;
        let key = |bounds| probe::key(bounds, RECTANGLE_COLUMNS);
        Ok(Box::new(joined.map(move |pair| pair.map(|((left, l), (right, r))| ((key(left), l), (key(right), r))))))
    }
}
