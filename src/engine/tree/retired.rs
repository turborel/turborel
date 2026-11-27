//! \[DESIGN\] The pages taken out of a tree and not yet freed, noted in its
//! meta page as well as in its epochs (RULES.md D814), so a crash loses
//! none of them for good. The epochs' list lives in memory only: a page
//! taken out before a checkpoint and freed after it would be, in the
//! checkpoint's image, neither reached by the tree nor free. A tree opened
//! takes the pages its meta page notes as its own to free, as no
//! operation of the run before can hold them.
//!
//! A page is noted after the tree stops reaching it and unnoted before it
//! is freed, so a meta page never notes a page the tree reaches, nor one
//! freed and given again: a checkpoint between costs a page of space, no
//! more. A meta page too full to note one more costs the same.
//!
//! ```text
//! meta page: format u8, root u64, then, if any are noted, their count
//!   u16 and each page u64, integers little-endian
//! ```

use super::{BLinkTree, META_LEN, corrupt};
use crate::disk::pager::PageId;
use crate::error::Result;

/// Bytes of the count of pages noted.
const COUNT: usize = 2;
/// Bytes of each page noted.
const PAGE: usize = 8;

impl BLinkTree {
    /// Notes `pages`, taken out, in the meta page, as many as fit.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if the meta page is malformed.
    pub(super) fn note_retired(&self, pages: &[PageId]) -> Result<()> {
        let _meta = self.lock_meta()?;
        let mut content = self.pool.write(self.file, self.meta)?;
        let mut noted = noted(&content)?;
        let room = self.capacity.saturating_sub(META_LEN.saturating_add(COUNT)).checked_div(PAGE).unwrap_or_default();
        noted.extend(pages.iter().take(room.saturating_sub(noted.len())));
        set_noted(&mut content, &noted)
    }

    /// Unnotes `pages` in the meta page, before they are freed.
    ///
    /// # Errors
    ///
    /// As `note_retired`.
    pub(super) fn unnote_retired(&self, pages: &[PageId]) -> Result<()> {
        let _meta = self.lock_meta()?;
        let mut content = self.pool.write(self.file, self.meta)?;
        let mut noted = noted(&content)?;
        noted.retain(|page| !pages.contains(page));
        set_noted(&mut content, &noted)
    }
}

/// The pages meta page `content` notes.
///
/// # Errors
///
/// SQLCODE -916 if its list runs past its end.
pub(super) fn noted(content: &[u8]) -> Result<Vec<PageId>> {
    let Some(list) = content.get(META_LEN..).filter(|list| !list.is_empty()) else { return Ok(Vec::new()) };
    let (count, pages) = list.split_at_checked(COUNT).ok_or_else(|| corrupt("a meta page's count of pages is cut short"))?;
    let count = usize::from(u16::from_le_bytes(<[u8; COUNT]>::try_from(count).map_err(|_| corrupt("a meta page's count"))?));
    if pages.len() != count.saturating_mul(PAGE) {
        return Err(corrupt("a meta page's pages are not as many as it counts"));
    }
    Ok(pages.as_chunks::<PAGE>().0.iter().map(|page| PageId(u64::from_le_bytes(*page))).collect())
}

/// Makes meta page `content` note `pages`, its format and root kept.
fn set_noted(content: &mut Vec<u8>, pages: &[PageId]) -> Result<()> {
    content.truncate(META_LEN);
    if pages.is_empty() {
        return Ok(());
    }
    let count = u16::try_from(pages.len()).map_err(|_| corrupt("a meta page notes too many pages"))?;
    content.extend_from_slice(&count.to_le_bytes());
    for page in pages {
        content.extend_from_slice(&page.0.to_le_bytes());
    }
    Ok(())
}

/// Makes meta page `content` name `root`, the pages it notes kept.
///
/// # Errors
///
/// SQLCODE -916 if it is shorter than a meta page.
pub(super) fn set_root(content: &mut [u8], root: PageId) -> Result<()> {
    content.get_mut(1..META_LEN).ok_or_else(|| corrupt("a meta page is cut short"))?.copy_from_slice(&root.0.to_le_bytes());
    Ok(())
}
