//! \[DESIGN\] Reads by key that start where the last one ended (RULES.md
//! D31). The rows of a range sit side by side, so a scan through an index
//! asks for key after key of one leaf, and a descent from the root for each
//! would latch the root and the inner nodes, which every thread shares,
//! for an answer the leaf already holds.
//!
//! The leaf is trusted only if it cannot have left the tree. A leaf leaves
//! by being retired (`reclaim.rs`), which moves the tree's stamp first. A
//! read enters its epoch, then loads the stamp: unchanged, nothing retired
//! before that load has been freed behind it, and whatever is retired after
//! it stays unfreed until the read leaves its epoch. A changed stamp costs
//! one descent.
//!
//! A leaf holds no keys below its lowest, so one in the tree whose range
//! covers a key at or above its first entry is the leaf for that key, split
//! or not, the key present or not. A key past the leaf's range is looked
//! for in its right neighbour, as a descent moves right (Lehman and Yao
//! 1981, §4): a link read from a leaf in the tree names no page freed, as
//! a leaf is unlinked before it is retired. Any other read descends.

use super::node::Node;
use super::{BLinkTree, Path, Target};
use crate::error::Result;
use crate::storage::Near;

/// \[DESIGN\] The misses in a row after which a scan stops trying its leaf:
/// its keys are not side by side, and each try costs a page read.
const MOST_MISSES: u8 = 8;
/// \[DESIGN\] The leaves a read tries before it descends: the one it knows
/// and its right neighbour, where a scan's next key goes when it leaves one.
const LEAVES_TRIED: usize = 2;

impl BLinkTree {
    /// Whether `key` is in the tree; its value left in `near`, which is
    /// kept from read to read of one scan.
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn get_near(&self, key: &[u8], near: &mut Near) -> Result<bool> {
        let _guard = self.epochs.guard();
        let stamp = self.epochs.stamp();
        if let Some(found) = self.in_leaf(key, stamp, near)? {
            return Ok(found);
        }
        let bytes = &mut near.bytes;
        let mut path = Path { tallied: true, ..Path::default() };
        let (leaf, found) = self.descend(Target::Key(key), 0, &mut path, |leaf| copied(leaf, key, bytes))?;
        near.leaf = Some((leaf.0, stamp));
        Ok(found)
    }

    /// Whether `key` is in the tree, if the leaf `near` remembers answers,
    /// or the one right of it, for a key past its range.
    fn in_leaf(&self, key: &[u8], stamp: u64, near: &mut Near) -> Result<Option<bool>> {
        let Some((first, _)) = near.leaf.filter(|&(_, at)| at == stamp && near.misses < MOST_MISSES) else { return Ok(None) };
        let mut next = Some(first);
        for _ in 0..LEAVES_TRIED {
            let Some(leaf) = next else { break };
            let page = self.pool.read_tallied(self.file, leaf, Some(&self.tally))?;
            let node = Node::parse(&page)?;
            if answers(&node, key)? {
                self.tally.lookup();
                near.misses = 0;
                near.leaf = Some((leaf, stamp));
                return copied(&node, key, &mut near.bytes).map(Some);
            }
            next = node.right.filter(|_| node.level == 0 && !node.deleted && !node.covers(key)).map(|right| right.0);
        }
        near.misses = near.misses.saturating_add(1);
        Ok(None)
    }
}

/// Whether leaf `node` is the one for `key`.
fn answers(node: &Node<'_>, key: &[u8]) -> Result<bool> {
    Ok(node.level == 0 && !node.deleted && node.len() > 0 && node.covers(key) && node.key(0)? <= key)
}

/// Whether `leaf` holds `key`, its value copied to `bytes`.
fn copied(leaf: &Node<'_>, key: &[u8], bytes: &mut Vec<u8>) -> Result<bool> {
    bytes.clear();
    match leaf.search(key)? {
        Ok(at) => {
            bytes.extend_from_slice(leaf.entry(at)?.1);
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}
