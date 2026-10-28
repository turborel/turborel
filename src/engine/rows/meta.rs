//! \[DESIGN\] What finds a table of the row store (RULES.md D31): its meta
//! page, which names its tree and the row ids reserved, and its locator,
//! which the catalog keeps (`StorageEngine::open_table`).
//!
//! ```text
//! meta page:  format u8 (TABLE_FORMAT) | tree's meta page u64 | ids reserved u64
//! locator:    meta page u64 | durable position u64
//! ```
//!
//! Integers are little-endian.

use crate::disk::pager::PageId;

/// The format of a table's meta page.
pub const TABLE_FORMAT: u8 = 1;
const META_LEN: usize = 17;
/// Bytes of a page number or a position.
const WORD: usize = 8;

/// A table's locator: its meta page, and the log position its pages hold
/// every change through, both little-endian.
pub(super) fn locator_of(meta: PageId, durable: u64) -> Vec<u8> {
    [meta.0.to_le_bytes(), durable.to_le_bytes()].concat()
}

/// The meta page and durable position of a locator.
pub(super) fn parse_locator(locator: &[u8]) -> Option<(PageId, u64)> {
    let (meta, durable) = locator.split_at_checked(8)?;
    Some((PageId(u64::from_le_bytes(meta.try_into().ok()?)), u64::from_le_bytes(durable.try_into().ok()?)))
}

/// The content of a table's meta page.
pub(super) fn meta_content(tree: PageId, reserved: u64) -> Vec<u8> {
    let mut content = Vec::with_capacity(META_LEN);
    content.push(TABLE_FORMAT);
    content.extend_from_slice(&tree.0.to_le_bytes());
    content.extend_from_slice(&reserved.to_le_bytes());
    content
}

/// The tree's meta page and the ids reserved, if `content` is a table's
/// meta page.
pub(super) fn parse_meta(content: &[u8]) -> Option<(PageId, u64)> {
    let [TABLE_FORMAT, tree @ .., _, _, _, _, _, _, _, _] = <[u8; META_LEN]>::try_from(content).ok()? else { return None };
    let reserved = content.get(WORD.saturating_add(1)..).and_then(|b| <[u8; 8]>::try_from(b).ok())?;
    Some((PageId(u64::from_le_bytes(tree)), u64::from_le_bytes(reserved)))
}
