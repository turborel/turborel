//! \[DESIGN\] An R-link tree packed bottom-up by Sort-Tile-Recursive, after
//! Leutenegger, Lopez and Edgington, "STR: A Simple and Efficient
//! Algorithm for R-Tree Packing", ICDE 1997 (RULES.md D139). The entries
//! are sorted by the centre of their X extent in a work area
//! (`spill::sorted_records`), cut into vertical slabs of about the square
//! root of the leaf count, each slab sorted again by the centre of its Y
//! extent, and its runs of `FILL` of a node written as leaves. Each level
//! above is tiled the same way, in memory, from one entry for each node
//! below, up to a single root. Identical entries become one counted entry.
//!
//! The packed tree is made beside the tree, unreachable until done. Then,
//! with every writer of the tree held out by its row locks, the entries
//! the tree holds, written while the load ran, are inserted into it, and
//! the meta page names its root. The old nodes are retired, freed once no
//! scan can still reach them. So a load works on a tree that writers keep
//! changing, as CREATE INDEX's build does, and a failed load changes
//! nothing. Each work area gets half of the one given, since a slab is
//! sorted while the first sort's records are still being read.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::node::{Entry, MAX_LEVEL, Node};
use super::scan::Walk;
use super::{Meta, RTree, corrupt, meta_bytes};
use crate::disk::pager::PageId;
use crate::error::{Result, SqlError};
use crate::spatial::{Bounds, DIMENSIONS};
use crate::spill::{Area, Records, WorkAreas, sorted_records};
use crate::storage::RowId;
use crate::sync::lock_builtin;

/// \[DESIGN\] How full a packed node is made: nine tenths of its capacity,
/// so the inserts that follow fill it before it splits, as a B-link tree's
/// loaded nodes are.
const FILL: (usize, usize) = (9, 10);
/// A record's words: two sort keys, four coordinates, the row and its
/// count.
const WORD: usize = 8;
const LOWER_X: usize = 2;
const LOWER_Y: usize = 3;
const UPPER_X: usize = 4;
const UPPER_Y: usize = 5;
const ROW: usize = 6;
const COUNT: usize = 7;
/// The bytes two records of one entry share: all but the count.
const IDENTITY: usize = WORD * COUNT;
const RECORD: usize = IDENTITY + WORD;
/// Flipping a float's sign bit, or all bits of a negative one, orders its
/// bits as the number is ordered.
const SIGN: u64 = 1 << 63;
const X: usize = 0;
const Y: usize = 1;

/// Entries a packed node holds, at least two so the tree narrows.
pub(super) fn fill(capacity: usize) -> usize {
    capacity.saturating_mul(FILL.0).checked_div(FILL.1).unwrap_or_default().max(2)
}

/// The packed nodes made so far, and the next stamp to give.
struct Packer<'a> {
    tree: &'a RTree,
    next: u64,
    fill: usize,
    pages: Vec<PageId>,
}

impl RTree {
    /// Adds every entry of `entries` as one packed load. Entries that
    /// writers add or remove meanwhile are kept. The tree's own are moved
    /// into the packed tree before it takes the tree's place.
    ///
    /// # Errors
    ///
    /// The entries' own; the work area's, SQLCODE -918 if the budget cannot
    /// grant it; storage errors. A failed load changes nothing.
    pub fn load(&self, entries: &mut dyn Iterator<Item = Result<(Bounds, RowId)>>, (areas, area): (&WorkAreas, Area)) -> Result<()> {
        let half = Area::new(area.bytes.checked_div(2).unwrap_or_default(), area.batch);
        let first = self.meta_fields()?.next;
        let side = RTree::create_from(Arc::clone(&self.inner.pool), self.inner.file, first)?;
        let loaded = side.pack(entries, (areas, half), first).and_then(|()| self.absorb(&side));
        if loaded.is_err() {
            // Nothing links to the side tree; failing to free it leaks
            // pages only, and the load's own error is the one to report.
            let _ = side.free();
        }
        loaded
    }

    fn pack(&self, entries: &mut dyn Iterator<Item = Result<(Bounds, RowId)>>, work: (&WorkAreas, Area), first: u64) -> Result<()> {
        let mut count = 0_usize;
        let mut encoded = entries.map(|entry| {
            count = count.saturating_add(1);
            entry.map(|(bounds, row)| encode(&Entry { bounds, id: row, value: 1 }, X))
        });
        let sorted = sorted_records(work.0, work.1, &mut encoded)?;
        drop(encoded);
        let mut merged = coalesced(sorted);
        let mut packer = Packer { tree: self, next: first.saturating_add(1), fill: fill(self.inner.capacity), pages: Vec::new() };
        let built = packer.leaves(&mut merged, count, work).and_then(|leaves| packer.upper(leaves));
        match built.and_then(|root| self.publish(root, packer.next)) {
            Ok(()) => Ok(()),
            Err(error) => {
                packer.discard();
                Err(error)
            }
        }
    }

    /// Names `root` in the meta page, the side tree's empty first root
    /// freed, as nothing has read it.
    fn publish(&self, root: Option<Entry>, next: u64) -> Result<()> {
        let meta = self.meta_fields()?;
        let Some(root) = root else { return Ok(()) };
        *self.inner.pool.write(self.inner.file, self.inner.meta)? = meta_bytes(Meta { root: PageId(root.id), stamp: root.value, next });
        self.inner.pool.free(self.inner.file, meta.root)
    }

    /// Moves this tree's entries into `side`, with every writer held out,
    /// and puts `side`'s root in its place; its old nodes are retired.
    fn absorb(&self, side: &RTree) -> Result<()> {
        let _writers: Vec<_> = self.inner.rows.iter().map(lock_builtin).collect();
        let pin = self.pin();
        let mut old = Vec::new();
        let mut walk = Walk::new(self.clone(), None);
        while let Some((id, node)) = walk.node()? {
            old.push(id);
            if node.level == 0 {
                node.entries.iter().try_for_each(|e| side.insert_counted(e.bounds, e.id, e.value))?;
            }
        }
        drop(walk);
        let packed = side.meta_fields()?;
        {
            let _meta = self.inner.locks.lock(self.inner.meta)?;
            let mine = self.meta_fields()?;
            let next = mine.next.max(packed.next);
            *self.inner.pool.write(self.inner.file, self.inner.meta)? = meta_bytes(Meta { next, ..packed });
        }
        for id in old {
            self.inner.epochs.retire(id);
            self.inner.retired.fetch_add(1, Ordering::Relaxed);
        }
        drop(pin);
        // The side tree's meta page is known to no one else, and failing
        // to free it, or a retired page, costs space only.
        let _ = self.inner.pool.free(self.inner.file, side.inner.meta);
        let _ = self.release();
        Ok(())
    }
}

impl Packer<'_> {
    /// Leaves of slabs of entries in X order, each slab in Y order.
    fn leaves(&mut self, entries: &mut dyn Iterator<Item = Result<Entry>>, count: usize, work: (&WorkAreas, Area)) -> Result<Vec<Entry>> {
        let per_slab = self.per_slab(count);
        let mut made = Vec::new();
        loop {
            let mut slab = (&mut *entries).take(per_slab).map(|entry| entry.map(|e| encode(&e, Y)));
            let mut sorted = sorted_records(work.0, work.1, &mut slab)?.peekable();
            if sorted.peek().is_none() {
                return Ok(made);
            }
            let mut node = Vec::with_capacity(self.fill);
            for record in sorted {
                node.push(decode(&record?)?);
                if node.len() >= self.fill {
                    made.push(self.node(0, std::mem::take(&mut node))?);
                }
            }
            if !node.is_empty() {
                made.push(self.node(0, node)?);
            }
        }
    }

    /// Entries in a slab: `fill` times the nodes of a slab, the slabs about
    /// the square root of the nodes.
    fn per_slab(&self, count: usize) -> usize {
        let nodes = count.div_ceil(self.fill).max(1);
        let root = nodes.isqrt();
        let slabs = if root.saturating_mul(root) < nodes { root.saturating_add(1) } else { root };
        self.fill.saturating_mul(nodes.div_ceil(slabs.max(1)))
    }

    /// The levels above `below`, tiled in memory, up to one root.
    fn upper(&mut self, mut below: Vec<Entry>) -> Result<Option<Entry>> {
        let mut level = 0_u8;
        while below.len() > 1 {
            level = level.checked_add(1).filter(|&l| l <= MAX_LEVEL).ok_or_else(|| corrupt("a packed tree is too tall"))?;
            let per_slab = self.per_slab(below.len());
            below.sort_by(|a, b| centre(a.bounds, X).total_cmp(&centre(b.bounds, X)));
            let mut up = Vec::new();
            for slab in below.chunks_mut(per_slab) {
                slab.sort_by(|a, b| centre(a.bounds, Y).total_cmp(&centre(b.bounds, Y)));
                for group in slab.chunks(self.fill) {
                    up.push(self.node(level, group.to_vec())?);
                }
            }
            below = up;
        }
        Ok(below.pop())
    }

    /// A new node of `entries` at `level`, and its parent's entry for it.
    fn node(&mut self, level: u8, entries: Vec<Entry>) -> Result<Entry> {
        let stamp = self.next;
        self.next = stamp.checked_add(1).ok_or_else(|| corrupt("its split stamps are exhausted"))?;
        let mut node = Node::empty(stamp, level);
        node.entries = entries;
        let bounds = node.bounds().ok_or_else(|| corrupt("a packed node has no entries"))?;
        let (id, mut page) = self.tree.inner.pool.new_page(self.tree.inner.file)?;
        self.pages.push(id);
        *page = node.bytes()?;
        Ok(Entry { bounds, id: id.0, value: stamp })
    }

    /// Frees the nodes made, which nothing links to after a failure. A
    /// failure to free leaks a page only.
    fn discard(&mut self) {
        for id in self.pages.drain(..) {
            let _ = self.tree.inner.pool.free(self.tree.inner.file, id);
        }
    }
}

/// Records of one entry, adjacent once sorted, as one counted entry.
fn coalesced(records: Records) -> impl Iterator<Item = Result<Entry>> {
    let mut records = records.peekable();
    std::iter::from_fn(move || {
        let first = match records.next()? {
            Ok(first) => first,
            Err(error) => return Some(Err(error)),
        };
        let mut entry = match decode(&first) {
            Ok(entry) => entry,
            Err(error) => return Some(Err(error)),
        };
        while let Some(next) = records.next_if(|next| next.as_ref().is_ok_and(|n| n.get(..IDENTITY) == first.get(..IDENTITY))) {
            let added = next.and_then(|n| decode(&n)).map(|n| n.value).unwrap_or_default();
            entry.value = entry.value.saturating_add(added);
        }
        Some(Ok(entry))
    })
}

/// The centre of `bounds` on `axis`, halves added so it cannot overflow.
fn centre(bounds: Bounds, axis: usize) -> f64 {
    let (lower, upper) = (bounds.lower(), bounds.upper());
    lower.get(axis).copied().unwrap_or_default() / 2.0_f64 + upper.get(axis).copied().unwrap_or_default() / 2.0_f64
}

/// Big-endian bits that sort as the number does.
fn sort_key(value: f64) -> u64 {
    let bits = value.to_bits();
    if bits & SIGN == 0 { bits | SIGN } else { !bits }
}

/// An entry as a record sorted by its centre on `first`, then the other.
fn encode(entry: &Entry, first: usize) -> Vec<u8> {
    let other = DIMENSIONS.saturating_sub(1).saturating_sub(first);
    let mut words = vec![sort_key(centre(entry.bounds, first)), sort_key(centre(entry.bounds, other))];
    words.extend(entry.bounds.lower().into_iter().chain(entry.bounds.upper()).map(f64::to_bits));
    words.extend([entry.id, entry.value]);
    words.into_iter().flat_map(u64::to_be_bytes).collect()
}

fn decode(record: &[u8]) -> Result<Entry> {
    let word = |at: usize| -> Result<u64> {
        let start = at.saturating_mul(WORD);
        let bytes = record.get(start..start.saturating_add(WORD)).and_then(|b| <[u8; WORD]>::try_from(b).ok());
        bytes.map(u64::from_be_bytes).ok_or_else(bad_record)
    };
    if record.len() != RECORD {
        return Err(bad_record());
    }
    let coordinate = |at: usize| word(at).map(f64::from_bits);
    let bounds = Bounds::new([coordinate(LOWER_X)?, coordinate(LOWER_Y)?], [coordinate(UPPER_X)?, coordinate(UPPER_Y)?])?;
    Ok(Entry { bounds, id: word(ROW)?, value: word(COUNT)? })
}

fn bad_record() -> SqlError {
    corrupt("a packed load's sorted record is malformed")
}
