//! \[DESIGN\] What finds a table of the row store (RULES.md D31): its meta
//! page, which names its tree, the row ids reserved, its frozen row
//! groups and its zone maps, and its locator, which the catalog keeps
//! (`StorageEngine::open_table`).
//!
//! ```text
//! meta page:  format u8 (TABLE_FORMAT) | tree's meta page u64 |
//!             ids reserved u64 | the first page of its groups' directory
//!             u64, 0 for none (`rows/frozen.rs`) | the page listing
//!             its zone maps' pages u64, 0 for none (`rows/zones/saved.rs`)
//! locator:    meta page u64 | durable position u64
//! ```
//!
//! Integers are little-endian.

use crate::disk::pager::PageId;

/// The format of a table's meta page.
pub const TABLE_FORMAT: u8 = 1;
const META_LEN: usize = 33;
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

/// What a table's meta page names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Meta {
    /// The tree's meta page.
    pub(super) tree: PageId,
    /// The row ids reserved.
    pub(super) reserved: u64,
    /// The first page of the groups' directory, 0 for none.
    pub(super) directory: u64,
    /// The page listing its zone maps' pages, 0 for none.
    pub(super) zones: u64,
}

/// The content of a table's meta page.
pub(super) fn meta_content(meta: Meta) -> Vec<u8> {
    let mut content = Vec::with_capacity(META_LEN);
    content.push(TABLE_FORMAT);
    for word in [meta.tree.0, meta.reserved, meta.directory, meta.zones] {
        content.extend_from_slice(&word.to_le_bytes());
    }
    content
}

/// What `content` names, if it is a table's meta page.
pub(super) fn parse_meta(content: &[u8]) -> Option<Meta> {
    let word = |at: usize| content.get(at..at.checked_add(WORD)?).and_then(|b| <[u8; WORD]>::try_from(b).ok()).map(u64::from_le_bytes);
    let zones = match (content.first().copied(), content.len()) {
        (Some(TABLE_FORMAT), META_LEN) => word(1 + 3 * WORD)?,
        (Some(_) | None, _) => return None,
    };
    Some(Meta { tree: PageId(word(1)?), reserved: word(1 + WORD)?, directory: word(1 + 2 * WORD)?, zones })
}
