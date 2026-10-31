//! \[DESIGN\] Where an instance of a temporary table keeps the older
//! versions its transactions keep (RULES.md D49): a log whose records are
//! pages of the instance's own scratch file (`db/temporary/pages.rs`), so
//! they share its pool of MEMORY.WORK_BYTES reserved of the budget, go to
//! the file as the pool evicts them, through the encodings spill files
//! take, and are removed with it: never logged, never recovered. The undo
//! store over it (`engine/rows/undo.rs`) gathers versions in batches of a
//! page's content, so a record is mostly one page. Split from
//! db/temporary/storage.rs (RULES.md R-SIZE-2).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::log::{Log, LogCapabilities, Lsn, Records};
use crate::sync::lock_builtin;

/// \[DESIGN\] A log in pages of a scratch file: each record a page of its
/// own (with overflow pages if longer than one holds), found by its
/// position; positions are 1, 2, 3, ..., as `MemoryLog`'s. Not durable:
/// it lasts as long as its file.
pub(super) struct PagedLog {
    pool: Arc<BufferPool>,
    file: FileId,
    state: Mutex<State>,
}

/// The pages of a log's records still kept, by position, and the last
/// position given.
#[derive(Default)]
struct State {
    pages: BTreeMap<Lsn, PageId>,
    last: u64,
}

impl PagedLog {
    /// An empty log in pager file `file` of `pool`.
    pub(super) fn new(pool: Arc<BufferPool>, file: FileId) -> PagedLog {
        PagedLog { pool, file, state: Mutex::default() }
    }

    /// The content of page `page`.
    fn content(&self, page: PageId) -> Result<Vec<u8>> {
        Ok(self.pool.read(self.file, page.0)?.to_vec())
    }
}

impl Log for PagedLog {
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: false }
    }

    /// The page written under the state's lock, so positions follow the
    /// order of appends.
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        let mut state = lock_builtin(&self.state);
        let last = state.last.checked_add(1).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the log has no position left"))?;
        let (page, mut content) = self.pool.new_page(self.file)?;
        *content = record.to_vec();
        drop(content);
        state.last = last;
        state.pages.insert(Lsn(last), page);
        Ok(Lsn(last))
    }

    fn sync(&self, through: Lsn) -> Result<()> {
        if through.0 > lock_builtin(&self.state).last {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] the log has no position {} yet", through.0)));
        }
        Ok(())
    }

    /// The pages taken as it is called, each read as its record is.
    fn read_from(&self, from: Lsn) -> Records<'_> {
        let pages: Vec<(Lsn, PageId)> = lock_builtin(&self.state).pages.range(from..).map(|(&at, &page)| (at, page)).collect();
        Box::new(pages.into_iter().map(move |(at, page)| Ok((at, self.content(page)?))))
    }

    fn read_at(&self, at: Lsn) -> Result<Option<Vec<u8>>> {
        let page = lock_builtin(&self.state).pages.get(&at).copied();
        page.map(|p| self.content(p)).transpose()
    }

    /// Each page freed in the file; one that cannot be stays, for a later
    /// call.
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        let mut state = lock_builtin(&self.state);
        while let Some((&at, &page)) = state.pages.first_key_value().filter(|(at, _)| **at < before) {
            self.pool.free(self.file, page)?;
            state.pages.remove(&at);
        }
        Ok(())
    }

    fn end(&self) -> Lsn {
        Lsn(lock_builtin(&self.state).last.saturating_add(1))
    }
}

#[cfg(test)]
mod tests;
