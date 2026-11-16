//! \[DESIGN\] Pages passed through the pool, never kept in a frame
//! (RULES.md D30, D79): a large object's chunks, each a page as long as a
//! chunk, written once and never changed. A frame is charged a page's
//! size against the memory budget, and a pool of frames each holding a
//! chunk would hold many times its budget, so such a page is written
//! straight to its file and read from it into memory its reader holds
//! alone, for as long as it reads it.

use std::sync::PoisonError;

use super::super::{BufferPool, FileId, Key, Pin};
use crate::disk::pager::{Extent, PageId};
use crate::error::Result;

impl BufferPool {
    /// A new page of pager file `file` holding `content`, written to the
    /// file, kept in no frame.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `file` is not a registered pager file; SQLCODE -918
    /// if it is kept in memory of a budget with no room for the page; the
    /// errors of writing it.
    pub fn write_passing(&self, file: FileId, content: &[u8]) -> Result<PageId> {
        let pager = self.pager(file)?;
        let id = pager.allocate(1).first().map_or(PageId(0), |e| e.start);
        match pager.reserve(id).and_then(|()| pager.write(id, content)) {
            Ok(()) => Ok(id),
            Err(e) => {
                // Never named, so nothing refers to it.
                let _ = pager.free(&[Extent { start: id, count: 1 }]);
                Err(e)
            }
        }
    }

    /// The content of page `id` of pager file `file`, as `write_passing`
    /// wrote it: from a frame if one holds it, else read from the file
    /// into memory of its own, kept in no frame.
    ///
    /// # Errors
    ///
    /// As `read`.
    pub fn read_passing(&self, file: FileId, id: PageId) -> Result<Vec<u8>> {
        let key = Key { file, id: id.0 };
        if let Some(pinned) = self.pin_mapped(key)?
            && let Some(frame) = self.settled_holding(pinned, key)?
        {
            let _pin = Pin(frame);
            return Ok(frame.content.read().unwrap_or_else(PoisonError::into_inner).clone());
        }
        let mut content = Vec::new();
        self.pager(file)?.read(id, &mut content)?;
        Ok(content)
    }
}
