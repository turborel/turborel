//! \[DESIGN\] Where a linear hash table (`engine/hash.rs`) keeps its shape
//! for `HashTable::open`: its directory pages, each bucket's first page in
//! order, and its meta page, which lists them with the shape and counts.
//! Split from hash.rs (RULES.md R-SIZE-2).

use super::{FORMAT, HashTable, Shape, corrupt};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::sync::lock_builtin;

impl HashTable {
    /// Writes the directory page of the last bucket of `shape`, adding a
    /// directory page or freeing an empty last one as it grew or shrank.
    pub(super) fn write_directory(&self, directory: &mut Vec<PageId>, shape: Shape) -> Result<()> {
        let per_page = self.capacity.checked_div(8).unwrap_or(1).max(1);
        let needed = usize::try_from(shape.buckets()).unwrap_or(usize::MAX).div_ceil(per_page);
        while directory.len() > needed {
            if let Some(d) = directory.pop() {
                self.pool.free(self.file, d)?;
            }
        }
        while directory.len() < needed {
            directory.push(Self::new_page(&self.pool, self.file, Vec::new())?);
        }
        let last = needed.saturating_sub(1);
        let mut ids = Vec::new();
        for b in u64::try_from(last.saturating_mul(per_page)).unwrap_or(u64::MAX)..shape.buckets() {
            ids.extend_from_slice(&self.head(b)?.0.to_le_bytes());
        }
        let page = directory.get(last).copied().ok_or_else(|| corrupt("it has no directory page"))?;
        *self.pool.write(self.file, page)? = ids;
        Ok(())
    }

    pub(super) fn write_meta(&self, directory: &[PageId], shape: Shape) -> Result<()> {
        let mut page = vec![FORMAT, shape.level];
        page.extend_from_slice(&shape.split.to_le_bytes());
        page.extend_from_slice(&self.len().to_le_bytes());
        page.extend_from_slice(&self.bytes().to_le_bytes());
        page.extend_from_slice(&u32::try_from(directory.len()).unwrap_or(u32::MAX).to_le_bytes());
        for d in directory {
            page.extend_from_slice(&d.0.to_le_bytes());
        }
        *self.pool.write(self.file, self.meta)? = page;
        Ok(())
    }

    /// Writes its shape and counts to its meta page, for `open`: done as it
    /// grows and shrinks, and to be done before it is opened again.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors.
    pub fn persist(&self) -> Result<()> {
        let mut directory = lock_builtin(&self.directory);
        let shape = self.current();
        self.write_directory(&mut directory, shape)?;
        self.write_meta(&directory, shape)
    }
}
