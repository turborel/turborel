//! \[DESIGN\] Bulk loading (RULES.md D31): a tree made from entries given in
//! key order, for an index built whole while nothing else reads or writes
//! it. Leaves are filled left to right to `FILL` of a page, each one's high
//! key the shortest that parts it from the next (`node::shortest_between`);
//! then each level above the same way, from the separators of the one
//! below, up to a new root. The first leaf is the empty tree's root, so the
//! meta page changes only once. A node is left room past `FILL`, so the
//! inserts that follow fill it before it splits.
//!
//! The leaves are written as they fill, one held in memory at a time; the
//! levels above are made from their children's pages and separators, held
//! in memory: about one entry for each node below.

use std::sync::atomic::Ordering;

use super::node::{self, CHILD, HEADER, Node, Shape};
use super::{BLinkTree, corrupt, meta_content};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};

/// \[DESIGN\] How full a loaded node is made: nine tenths of its page.
const FILL: (usize, usize) = (9, 10);

/// A node's entries: keys and values.
type Entries = Vec<(Vec<u8>, Vec<u8>)>;

/// The leaf being filled.
struct Filling {
    id: PageId,
    entries: Entries,
    bytes: usize,
}

impl Filling {
    /// The bytes it takes with an entry of `key`, of `size` bytes whole,
    /// added (`within`).
    fn with(&self, key: &[u8], size: usize) -> usize {
        within(&self.entries, self.bytes, (key, size))
    }
}

/// The bytes a node of `entries`, `bytes` with their keys whole, takes with
/// an entry of `key`, `size` bytes whole, added: its keys kept past the
/// prefix they share with its first, which they share, in key order, with
/// `key` too (RULES.md D500).
fn within(entries: &Entries, bytes: usize, (key, size): (&[u8], usize)) -> usize {
    let prefix = entries.first().map_or(0, |(first, _)| first.iter().zip(key).take_while(|(a, b)| a == b).count());
    let kept = entries.len().saturating_add(1).saturating_mul(prefix);
    bytes.saturating_add(size).saturating_add(prefix).saturating_sub(kept)
}

impl BLinkTree {
    /// Loads `entries`, in key order, keys unique, into the tree, which is
    /// empty, and which nothing else reads or writes until it returns.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the tree is not empty, or the keys are not in order;
    /// the entries' own; the buffer pool's.
    pub fn load(&self, entries: &mut dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>)>>) -> Result<()> {
        let root = PageId(self.root.load(Ordering::Acquire));
        let empty = { Node::parse(&self.pool.read(self.file, root.0)?).map(|n| n.level == 0 && n.len() == 0)? };
        if !empty {
            return Err(SqlError::new(Code::Api, "[DESIGN] a B-link tree is loaded only while it is empty"));
        }
        let target = self.capacity.saturating_mul(FILL.0).checked_div(FILL.1).unwrap_or_default();
        let (mut children, mut separators) = (vec![root], Vec::new());
        let mut leaf = Filling { id: root, entries: Vec::new(), bytes: HEADER };
        for entry in entries {
            let (key, value) = entry?;
            if leaf.entries.last().is_some_and(|(last, _)| *last >= key) {
                return Err(SqlError::new(Code::Api, "[DESIGN] entries loaded into a B-link tree are not in key order"));
            }
            let size = node::entry_size(&key, &value);
            // Room for the high key too, which is no longer than the last.
            if !leaf.entries.is_empty() && leaf.with(&key, size).saturating_add(key.len()) > target {
                let next = self.placeholder()?;
                let high = leaf.entries.last().map(|(last, _)| node::shortest_between(last, &key).to_vec()).unwrap_or_default();
                self.write_node(
                    leaf.id,
                    &Shape { level: 0, high: Some(&high), right: Some(next), first: PageId(0), tree: self.meta.0 },
                    &leaf.entries,
                )?;
                children.push(next);
                separators.push(high);
                leaf = Filling { id: next, entries: Vec::new(), bytes: HEADER };
            }
            leaf.bytes = leaf.bytes.saturating_add(size);
            leaf.entries.push((key, value));
        }
        self.write_node(leaf.id, &Shape { level: 0, high: None, right: None, first: PageId(0), tree: self.meta.0 }, &leaf.entries)?;
        let mut level = 0_u8;
        while children.len() > 1 {
            level = level.checked_add(1).ok_or_else(|| corrupt("it is too tall"))?;
            (children, separators) = self.load_level(level, &children, &separators, target)?;
        }
        let top = children.first().copied().unwrap_or(root);
        if top != root {
            *self.pool.write(self.file, self.meta)? = meta_content(top);
            self.root.store(top.0, Ordering::Release);
        }
        Ok(())
    }

    /// The nodes of `level` over `children`, `separators[i]` parting child
    /// `i` from the next: their pages, and the separators parting them.
    fn load_level(&self, level: u8, children: &[PageId], separators: &[Vec<u8>], target: usize) -> Result<(Vec<PageId>, Vec<Vec<u8>>)> {
        // Room for the high key, a separator of this level.
        let room = separators.iter().map(Vec::len).max().unwrap_or_default();
        let mut nodes: Vec<(PageId, Entries)> = Vec::new();
        let mut up = Vec::new();
        let (mut first, mut entries, mut bytes) = (children.first().copied().unwrap_or(PageId(0)), Vec::new(), HEADER);
        for (separator, child) in separators.iter().zip(children.iter().skip(1)) {
            let size = node::entry_size(separator, &[0; CHILD]);
            if within(&entries, bytes, (separator, size)).saturating_add(room) > target {
                // The separator goes up, and the child starts the next node.
                nodes.push((first, std::mem::take(&mut entries)));
                up.push(separator.clone());
                (first, bytes) = (*child, HEADER);
                continue;
            }
            bytes = bytes.saturating_add(size);
            entries.push((separator.clone(), child.0.to_le_bytes().to_vec()));
        }
        nodes.push((first, entries));
        let pages = nodes.iter().map(|_| self.placeholder()).collect::<Result<Vec<_>>>()?;
        for (n, ((first_child, node_entries), id)) in nodes.iter().zip(&pages).enumerate() {
            let (high, right) = (up.get(n).map(Vec::as_slice), pages.get(n.saturating_add(1)).copied());
            let shape = Shape { level, high, right, first: *first_child, tree: self.meta.0 };
            self.write_node(*id, &shape, node_entries)?;
        }
        Ok((pages, up))
    }

    /// A new page, an empty leaf until it is written.
    fn placeholder(&self) -> Result<PageId> {
        let (id, mut page) = self.pool.new_page(self.file)?;
        *page = node::build(&Shape { level: 0, high: None, right: None, first: PageId(0), tree: self.meta.0 }, &[])?;
        Ok(id)
    }

    /// Writes node `id` of `shape` holding `entries`.
    fn write_node(&self, id: PageId, shape: &Shape<'_>, entries: &[(Vec<u8>, Vec<u8>)]) -> Result<()> {
        let refs: Vec<(&[u8], &[u8])> = entries.iter().map(|(k, v)| (k.as_slice(), v.as_slice())).collect();
        let content = node::build(shape, &refs)?;
        if content.len() > self.capacity {
            return Err(corrupt("a node loaded is larger than its page"));
        }
        *self.pool.write(self.file, id)? = content;
        Ok(())
    }
}
