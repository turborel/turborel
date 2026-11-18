//! \[DESIGN\] Node records in chunk pages: reading one, changing one under
//! its page's latch, and allocating and freeing node numbers. A node's
//! number is dense; freed numbers form a list through their first edge,
//! its head in the meta page, so slots are reused without a scan.

use std::sync::atomic::Ordering;

use crate::disk::pager::PageId;
use crate::error::Result;
use crate::sync::{lock_builtin, read, write};

use super::meta::NO_NODE;
use super::node::{FREE, Node, corrupt};
use super::{Alloc, DiskAnn};

impl DiskAnn {
    /// The page and offset of node `id`'s record.
    pub(super) fn locate(&self, id: u32) -> Result<(u64, usize)> {
        let (chunk, offset) = self.inner.shape.place(id);
        Ok((read(&self.inner.nodes)?.page(chunk)?, offset))
    }

    /// Node `id`, read from its page and never the cache, since a writer
    /// must see the record as it is.
    ///
    /// # Errors
    ///
    /// The buffer pool's; SQLCODE -916 for a malformed record.
    pub(super) fn read_node(&self, id: u32, node: &mut Node) -> Result<()> {
        let (page, offset) = self.locate(id)?;
        let content = self.inner.pool.read(self.inner.file, page)?;
        self.inner.shape.decode(content.get(offset..).ok_or_else(|| corrupt("a node is past its page"))?, node)
    }

    /// Changes node `id` by `change` under its page's write latch, if it
    /// says it changed it; the hot-node cache follows. `change` must not
    /// call a plug-in (R-ERR-4).
    ///
    /// # Errors
    ///
    /// `change`'s; the buffer pool's.
    pub(super) fn modify(&self, id: u32, change: impl FnOnce(&mut Node) -> Result<bool>) -> Result<bool> {
        let (page, offset) = self.locate(id)?;
        let shape = self.inner.shape;
        let mut node = Node::default();
        let mut content = self.inner.pool.write(self.inner.file, PageId(page))?;
        if content.len() < shape.page_bytes() {
            content.resize(shape.page_bytes(), 0);
        }
        let record = content.get_mut(offset..).ok_or_else(|| corrupt("a node is past its page"))?;
        shape.decode(record, &mut node)?;
        if !change(&mut node)? {
            return Ok(false);
        }
        shape.encode(&node, record)?;
        drop(content);
        self.recache(id, &node)?;
        Ok(true)
    }

    /// Writes `node` as node `id`, whatever was there.
    pub(super) fn put(&self, id: u32, node: &Node) -> Result<()> {
        let mut fresh = Some(node.clone());
        self.modify(id, |n| {
            *n = fresh.take().unwrap_or_default();
            Ok(true)
        })?;
        Ok(())
    }

    /// A node number for a new node, the free list's head or the next,
    /// with a new chunk page when it starts one. The meta page records it.
    ///
    /// # Errors
    ///
    /// The buffer pool's; SQLCODE -900 past 2^32 - 1 nodes.
    pub(super) fn allocate(&self) -> Result<u32> {
        let mut alloc = lock_builtin(&self.inner.alloc);
        let id = if alloc.free == NO_NODE {
            let id = alloc.next;
            let next = id.checked_add(1).filter(|&n| n != NO_NODE).ok_or_else(|| {
                crate::error::SqlError::new(crate::error::Code::Unsupported, "[EXTENSION] a DiskANN index holds at most 2^32 - 2 nodes")
            })?;
            self.ensure_chunk(id)?;
            alloc.next = next;
            id
        } else {
            let id = alloc.free;
            let mut node = Node::default();
            self.read_node(id, &mut node)?;
            if !node.free() {
                return Err(corrupt("its free list names a node in use"));
            }
            alloc.free = node.neighbours.first().copied().unwrap_or(NO_NODE);
            id
        };
        // Registered before the lock is let go, so consolidation never
        // takes a node being inserted for one a failure left unfinished.
        lock_builtin(&self.inner.pending).insert(id);
        if let Err(error) = self.write_meta(*alloc) {
            lock_builtin(&self.inner.pending).remove(&id);
            return Err(error);
        }
        Ok(id)
    }

    /// Adds the chunk page node `id` needs, if it starts one.
    pub(super) fn ensure_chunk(&self, id: u32) -> Result<()> {
        let (chunk, _) = self.inner.shape.place(id);
        let mut nodes = write(&self.inner.nodes)?;
        while nodes.pages.len() <= chunk {
            let (page, mut content) = self.inner.pool.new_page(self.inner.file)?;
            *content = vec![0_u8; self.inner.shape.page_bytes()];
            drop(content);
            nodes.push(&self.inner.pool, self.inner.file, page.0)?;
        }
        Ok(())
    }

    /// Makes node `id` a free slot at the free list's head.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub(super) fn release(&self, id: u32) -> Result<()> {
        let mut alloc = lock_builtin(&self.inner.alloc);
        let next = alloc.free;
        let vector = vec![0.0_f32; self.inner.shape.dims];
        self.put(id, &Node { flags: FREE, row: 0, count: 0, vector, norm: 0.0, neighbours: vec![next] })?;
        alloc.free = id;
        self.write_meta(*alloc)
    }

    /// Writes the meta page now.
    pub(super) fn note(&self) -> Result<()> {
        let alloc: Alloc = *lock_builtin(&self.inner.alloc);
        self.write_meta(alloc)
    }

    /// Moves `n` nodes from live to dead (or back, if `n` is negative).
    pub(super) fn kill(&self, n: i64) {
        let (live, dead) = (&self.inner.live, &self.inner.dead);
        let m = n.unsigned_abs();
        if n >= 0 {
            dec(live, m);
            dead.fetch_add(m, Ordering::AcqRel);
        } else {
            dec(dead, m);
            live.fetch_add(m, Ordering::AcqRel);
        }
    }
}

/// Subtracts `n` from `counter`, saturating at zero.
pub(super) fn dec(counter: &std::sync::atomic::AtomicU64, n: u64) {
    let mut now = counter.load(Ordering::Acquire);
    while let Err(seen) = counter.compare_exchange_weak(now, now.saturating_sub(n), Ordering::AcqRel, Ordering::Acquire) {
        now = seen;
    }
}

impl DiskAnn {
    /// The row of every live node, once each, read a node at a time as
    /// taken: an index's every entry. A node allocated after it starts
    /// past those it had may or may not be given; an error of the pool is
    /// given once and ends it.
    pub fn entries(&self) -> Box<dyn Iterator<Item = Result<u64>> + Send> {
        let index = self.clone();
        let allocated = lock_builtin(&self.inner.alloc).next;
        let mut failed = false;
        Box::new((0..allocated).filter_map(move |id| {
            if failed {
                return None;
            }
            let mut node = Node::default();
            match index.read_node(id, &mut node) {
                Ok(()) => (node.live() && node.count > 0).then_some(Ok(node.row)),
                Err(error) => {
                    failed = true;
                    Some(Err(error))
                }
            }
        }))
    }
}
