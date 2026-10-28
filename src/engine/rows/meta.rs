//! \[DESIGN\] What finds a table of the row store (RULES.md D31): its meta
//! page, which names its tree, the row ids reserved and its frozen row
//! groups, and its locator, which the catalog keeps
//! (`StorageEngine::open_table`).
//!
//! ```text
//! meta page:  format u8 (TABLE_FORMAT) | tree's meta page u64 |
//!             ids reserved u64 | the first page of its groups' directory
//!             u64, 0 for none (`rows/frozen.rs`)
//! locator:    meta page u64 | durable position u64
//! ```
//!
//! A meta page of format 1 has no directory: its table has no groups.
//!
//! Integers are little-endian.

use crate::disk::pager::PageId;

/// The format of a table's meta page.
pub const TABLE_FORMAT: u8 = 2;
/// The format before, of no frozen row groups.
const NO_GROUPS: u8 = 1;
const META_LEN: usize = 25;
const OLD_META_LEN: usize = 17;
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
pub(super) fn meta_content(tree: PageId, reserved: u64, directory: u64) -> Vec<u8> {
    let mut content = Vec::with_capacity(META_LEN);
    content.push(TABLE_FORMAT);
    content.extend_from_slice(&tree.0.to_le_bytes());
    content.extend_from_slice(&reserved.to_le_bytes());
    content.extend_from_slice(&directory.to_le_bytes());
    content
}

/// The tree's meta page, the ids reserved and the first page of the
/// groups' directory, 0 for none, if `content` is a table's meta page.
pub(super) fn parse_meta(content: &[u8]) -> Option<(PageId, u64, u64)> {
    let word = |at: usize| content.get(at..at.checked_add(WORD)?).and_then(|b| <[u8; WORD]>::try_from(b).ok()).map(u64::from_le_bytes);
    let directory = match (content.first().copied(), content.len()) {
        (Some(TABLE_FORMAT), META_LEN) => word(1 + 2 * WORD)?,
        (Some(NO_GROUPS), OLD_META_LEN) => 0,
        (Some(_) | None, _) => return None,
    };
    Some((PageId(word(1)?), word(1 + WORD)?, directory))
}
