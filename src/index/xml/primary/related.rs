//! \[EXTENSION\] RULES.md D106: the primary's answer of a query's relation
//! of an element to a node, by the rows of the Infoset keyed by row and
//! ORDPATH [Pal et al. §3.1.1]. "A byte comparison of two ORDPATH labels
//! yields the relative order of the nodes", and a child's label extends
//! its parent's [§2.2], so the nodes of a row in a range of keys are those
//! of a range of places: an ancestor is the entry of a prefix of the
//! node's ORDPATH, sought; an element following the node is an entry after
//! its range, and one following it of its parent one after it before the
//! parent's range ends, of as many components; one preceding it is before
//! it, of no prefix of it, and one preceding it of its parent after the
//! parent's own entry, of as many components. Each range is read forward
//! until an element of the relation's paths is met.

use std::ops::Bound;
use std::sync::atomic;

use super::Answering;
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::index::xml::keys::{Cursor, ENTRIES, after};
use crate::index::xml::related::Relation;
use crate::index::xml::{Target, XmlRelated};
use crate::storage::RowId;

/// The octets each component of `ordpath` ends at.
fn ends(ordpath: &[u8]) -> Result<Vec<usize>> {
    let mut at = Cursor { bytes: ordpath, at: 0 };
    let mut ends = Vec::new();
    while at.at < ordpath.len() {
        at.ordinal()?;
        ends.push(at.at);
    }
    Ok(ends)
}

/// The key after every key beginning with `prefix`.
fn past(prefix: &[u8]) -> Bound<Vec<u8>> {
    after(prefix)
}

impl Answering<'_> {
    /// Whether an element of the query's relation, `related`, of the paths
    /// `paths`, stands in it to the node of ORDPATH `node` of `row`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a key is malformed; the tree's.
    pub(super) fn related_by(&self, (related, paths): (&XmlRelated, &[u64]), row: RowId, node: &[u8]) -> Result<bool> {
        let entries = [&[ENTRIES][..], &row.to_be_bytes()].concat();
        let ends = ends(node)?;
        let parent = ends.len().checked_sub(2).and_then(|n| ends.get(n)).and_then(|&end| node.get(..end)).unwrap_or_default();
        let attribute = self.query.lookup.pattern.target == Target::Attribute;
        let below = |of: &[u8]| [&entries[..], of].concat();
        let low = |of: &[u8]| Bound::Included([&below(of)[..], &[1]].concat());
        match related.relation {
            Relation::Ancestor | Relation::AncestorOrSelf => {
                let own = related.relation == Relation::AncestorOrSelf && !attribute;
                let reached = ends.len().saturating_sub(usize::from(!own));
                for &end in ends.iter().skip(1).take(reached.saturating_sub(1)) {
                    let at = below(node.get(..end).unwrap_or_default());
                    if self.met((Bound::Included([&at[..], &[0]].concat()), past(&[&at[..], &[0]].concat())), paths, None)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Relation::Following => {
                let (Bound::Excluded(from) | Bound::Included(from)) = past(&below(node)) else { return Ok(false) };
                self.met((Bound::Included(from), past(&entries)), paths, None)
            }
            Relation::FollowingSibling => {
                if attribute {
                    return Ok(false);
                }
                let (Bound::Excluded(from) | Bound::Included(from)) = past(&below(node)) else { return Ok(false) };
                self.met((Bound::Included(from), past(&below(parent))), paths, Some(ends.len()))
            }
            Relation::Preceding => self.preceding(&entries, (node, &ends), paths),
            Relation::PrecedingSibling => {
                if attribute {
                    return Ok(false);
                }
                self.met((low(parent), Bound::Excluded(below(node))), paths, Some(ends.len()))
            }
        }
    }

    /// Whether an element of `paths` is preceding the node of ORDPATH
    /// `node`, of components ending at `ends`, among the row's `entries`:
    /// before it, of no prefix of it.
    fn preceding(&self, entries: &[u8], (node, ends): (&[u8], &[usize]), paths: &[u64]) -> Result<bool> {
        let upper = [entries, node].concat();
        let scan = self.index.tree.scan(Bound::Included(entries), Bound::Excluded(upper.as_slice()), Direction::Forward);
        for found in scan {
            let (key, _) = found?;
            self.read.fetch_add(1, atomic::Ordering::Relaxed);
            let (ordpath, path) = fields(&key)?;
            let above = ends.iter().any(|&end| node.get(..end) == Some(ordpath));
            if !above && paths.contains(&path) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether an entry from `low` to `high` is of an element of `paths`,
    /// of `components` components if given; read until one is.
    fn met(&self, (low, high): (Bound<Vec<u8>>, Bound<Vec<u8>>), paths: &[u64], components: Option<usize>) -> Result<bool> {
        let scan = self.index.tree.scan(low.as_ref().map(Vec::as_slice), high.as_ref().map(Vec::as_slice), Direction::Forward);
        for found in scan {
            let (key, _) = found?;
            self.read.fetch_add(1, atomic::Ordering::Relaxed);
            let (ordpath, path) = fields(&key)?;
            if paths.contains(&path) && components.is_none_or(|n| ends(ordpath).is_ok_and(|e| e.len() == n)) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// The ORDPATH and the path's id of a primary's entry.
fn fields(key: &[u8]) -> Result<(&[u8], u64)> {
    let mut at = Cursor { bytes: key, at: 1 };
    at.row()?;
    let ordpath = at.ordpath()?;
    at.byte()?;
    Ok((ordpath, at.ordinal()?))
}
