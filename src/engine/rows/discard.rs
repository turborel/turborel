//! \[EXTENSION\] A dropped table's pages freed (`TableStorage::discard`,
//! ISO/IEC 9075:1992 §11.18 GR2): when the last reference to it goes, so no
//! reader of an earlier snapshot reads a page given to another use. Split
//! from rows.rs (RULES.md R-SIZE-2).
//!
//! Freed: the pages of the values each version of each row keeps out of
//! line, the newest in the tree and the older in the undo store; its
//! frozen row groups' pages and their directory's; its zone maps' pages;
//! then the tree's pages, and the table's meta page. A failure cannot be reported
//! from a `drop`: the pages are then lost to the file, which costs space
//! only. Versions a crash left in the pages name older ones the undo store
//! no longer has (`rows/recovery.rs`); those are not followed.

use std::ops::Bound;
use std::sync::atomic::Ordering;

use super::RowTable;
use super::ids::row_id;
use super::version::Version;
use crate::engine::tree::Direction;
use crate::error::Result;

impl Drop for RowTable {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.free_all();
        }
    }
}

impl RowTable {
    /// Frees every page of the table.
    fn free_all(&self) -> Result<()> {
        for entry in self.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward) {
            let (key, newest) = entry?;
            let id = row_id(&key)?;
            let mut bytes = newest;
            loop {
                let version = Version::parse(&bytes)?;
                if !version.deleted {
                    self.free_values(id, version.record);
                }
                let Some(older) = version.older.and_then(|at| self.undo.read(at).ok()) else { break };
                bytes = older;
            }
        }
        // Its groups' pages go with the last reader of them.
        self.retire_all()?;
        if self.frozen.stored() != 0 {
            let pages = self.group_pages()?;
            let (_, chain) = crate::engine::columns::group::read_chain(&pages, crate::disk::pager::PageId(self.frozen.stored()))?;
            pages.free(&chain);
        }
        self.zones.free_pages()?;
        self.tree.free()?;
        self.pool.free(self.file, self.meta)
    }
}
