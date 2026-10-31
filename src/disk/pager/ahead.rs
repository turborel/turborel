//! \[DESIGN\] Pages read ahead (RULES.md D30): several pages of a pager
//! file read in one read of the file, for the buffer pool's read-ahead
//! (`disk/buffer/pool/ahead.rs`). A read ahead guesses at the pages that
//! will be wanted, so a page it cannot take whole, as `read` would give
//! it, from the bytes read, is left to `read`. Split from pager.rs
//! (RULES.md R-SIZE-2).

use std::sync::atomic::Ordering;

use super::format::{self, NONE};
use super::{PageId, Pager, add};
use crate::error::Result;

impl Pager {
    /// The contents of pages `ids`, in ascending order, read in one read of
    /// the file from the first to the last: each as `read` gives it, or
    /// `None` for a page `read` would refuse, one it would read more of the
    /// file for (its overflow pages), or one never written.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the file cannot be read.
    pub fn read_ahead(&self, ids: &[PageId]) -> Result<Vec<Option<Vec<u8>>>> {
        let on_file = self.on_file.load(Ordering::Acquire);
        let first = ids.first().map(|id| id.0).filter(|&id| id < on_file);
        let last = ids.iter().map(|id| id.0).filter(|&id| id < on_file).max();
        let (Some(first), Some(last)) = (first, last) else { return Ok(ids.iter().map(|_| None).collect()) };
        let bytes = self.read_run(first, last.saturating_sub(first).saturating_add(1))?;
        Ok(ids.iter().map(|&id| self.page_in(&bytes, first, id)).collect())
    }

    /// The content of page `id` among `bytes`, the pages from `first`, as
    /// `read_ahead` takes it.
    fn page_in(&self, bytes: &[u8], first: u64, id: PageId) -> Option<Vec<u8>> {
        let at = usize::try_from(id.0.checked_sub(first)?).ok()?.checked_mul(self.page_size)?;
        let page = bytes.get(at..at.checked_add(self.page_size)?)?;
        self.check_in_use(id).ok()?;
        let body = self.unseal(id.0, page).ok()??;
        self.check_data(id, body).ok()?;
        let (_, listed, directory, _) = format::data_header(body)?;
        if !listed.is_empty() || directory != NONE {
            return None;
        }
        let mut stored = Vec::new();
        self.load(body, &mut stored).ok()?;
        self.known.set(id.0, &[]);
        add(&self.counters.reads, 1);
        if self.pipeline.is_empty() {
            return Some(stored);
        }
        self.pipeline.decode(id.0, &stored).ok().map(std::borrow::Cow::into_owned)
    }
}
