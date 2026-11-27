//! \[DESIGN\] Keys written in key order (RULES.md D1235): each leaf a run
//! of them falls in is reached and locked once for the keys it covers,
//! each key searched, shown to its change and written under the leaf's
//! latch as `BLinkTree::update_alone_into` writes one. A change that does
//! not fit splits the leaf, posted up the tree as one key's split is, and
//! the keys after it go down again. A reader sees each key as soon as it
//! is written, as it sees a key written alone.

use super::node::Node;
use super::write::alone_at;
use super::{BLinkTree, Path, Slot, Within};
use crate::error::Result;

/// What `BLinkTree::update_sorted` asks of each key's change: of the key
/// at that place in the keys, its old value, if there, and the buffer
/// its new value is written into.
pub type SortedChange<'a> = dyn FnMut(usize, Option<&[u8]>, &mut Vec<u8>) -> Result<()> + 'a;

impl BLinkTree {
    /// \[DESIGN\] Puts each of `keys`, in ascending order, each with the
    /// count of its first bytes that are its key's, the value `change`
    /// writes for it: whether each was alone, as `update_alone_into` says.
    /// Each leaf is locked once for the run of keys it covers.
    ///
    /// # Errors
    ///
    /// As `update_alone_into`.
    pub fn update_sorted(&self, keys: &[(&[u8], usize)], change: &mut SortedChange<'_>) -> Result<Vec<bool>> {
        let op = self.epochs.guard();
        let mut alone = Vec::with_capacity(keys.len());
        let mut value = Vec::new();
        while alone.len() < keys.len() {
            self.leaf_run((op.within(), keys), &mut alone, &mut value, change)?;
        }
        Ok(alone)
    }

    /// Writes the keys of `keys` from the first not in `alone` on, while
    /// the leaf of the first covers them and takes them with no split; a
    /// split ends the run with that key written.
    fn leaf_run(
        &self,
        (within, keys): (Within<'_>, &[(&[u8], usize)]),
        alone: &mut Vec<bool>,
        value: &mut Vec<u8>,
        change: &mut SortedChange<'_>,
    ) -> Result<()> {
        let from = alone.len();
        let Some(&(first, _)) = keys.get(from) else { return Ok(()) };
        let mut path = Path::default();
        let leaf = self.leaf_for_write(within, first, &mut path)?;
        let mut held = self.lock(within, leaf)?;
        let mut page = self.latched_covering(within, &mut held, first)?;
        for (n, &(key, prefix)) in keys.iter().enumerate().skip(from) {
            let (found, lone) = {
                let node = Node::parse(&page)?;
                if n > from && (!node.covers(key) || node.deleted) {
                    return Ok(());
                }
                let found = node.search(key)?;
                let old = match found {
                    Ok(at) => Some(node.value(at)?),
                    Err(_) => None,
                };
                value.clear();
                change(n, old, value)?;
                (found, alone_at(&node, found, key.get(..prefix).unwrap_or(key))?)
            };
            alone.push(lone);
            let slot = match found {
                Ok(at) => Slot::Replace(at),
                Err(at) => Slot::Insert(at),
            };
            self.fits(key, value)?;
            if let Some(copy) = self.change_latched(&held, &mut page, (key, value), slot)? {
                drop(page);
                let splits = self.split(held.id, &copy, (key, value), slot)?;
                return self.posted((within, &path), held, key, splits).map(drop);
            }
        }
        Ok(())
    }
}
