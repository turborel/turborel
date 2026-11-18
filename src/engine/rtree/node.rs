//! \[DESIGN\] Versioned R-link nodes. Both leaf and branch entries occupy
//! 48 bytes: four f64 bounds, an id and a count (leaf) or split stamp
//! (branch). Coordinates retain all their bits; there is no inward
//! rounding that could make a search miss an entry.

use crate::bytes::{Put, Reader};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::spatial::Bounds;

use super::corrupt;

/// The independently versioned R-link page format.
const FORMAT: u8 = 1;
/// Format, level, flags, stamp, right link and entry count.
pub(super) const HEADER: usize = 23;
/// Four coordinates followed by two u64 fields.
pub(super) const ENTRY_BYTES: usize = 48;
/// An absent sibling; the pager never allocates this page number.
const NO_PAGE: u64 = u64::MAX;
/// The flags byte: an unposted split, and a node taken out of the tree.
const PENDING: u8 = 1;
const RETIRED: u8 = 2;
/// A corrupt tree cannot cause an unbounded descent or cursor stack.
pub(super) const MAX_LEVEL: u8 = 63;
/// The bytes of each field of an entry: an f64's or a u64's.
const WORD: usize = 8;
/// An entry's fields: four coordinates, an id and a count or stamp.
const ENTRY_WORDS: usize = 6;

#[derive(Clone, Copy, Debug)]
pub(super) struct Entry {
    pub(super) bounds: Bounds,
    /// Row number at a leaf; child page number in a branch.
    pub(super) id: u64,
    /// Reference count at a leaf; expected split stamp in a branch.
    pub(super) value: u64,
}

pub(super) struct Node {
    pub(super) level: u8,
    pub(super) stamp: u64,
    pub(super) right: Option<PageId>,
    /// The right sibling has not yet been posted to the parent. Readers
    /// need only stamps; writers finish this split before adding entries.
    pub(super) pending: bool,
    /// Taken out of the tree by reclamation (`reclaim.rs`). Its entries
    /// are frozen for scans that still hold it, and writers start again.
    pub(super) retired: bool,
    pub(super) entries: Vec<Entry>,
}

impl Node {
    pub(super) fn empty(stamp: u64, level: u8) -> Node {
        Node { level, stamp, right: None, pending: false, retired: false, entries: Vec::new() }
    }

    #[cfg(test)]
    pub(super) fn parse(bytes: &[u8]) -> Result<Node> {
        let mut node = Node::empty(0, 0);
        node.parse_into(bytes)?;
        Ok(node)
    }

    /// Becomes the node `bytes` hold, reusing its entries' allocation. The
    /// header is read field by field; the entries, fixed 48-byte records,
    /// are read as whole 8-byte words, each checked as before: finite,
    /// ordered bounds, and a leaf entry's nonzero count.
    pub(super) fn parse_into(&mut self, bytes: &[u8]) -> Result<()> {
        let mut r = Reader::new(bytes, "R-link node");
        let (level, stamp, right, flags, count) = header(&mut r)?;
        let pending = flags & PENDING != 0;
        let expected = count.checked_mul(ENTRY_BYTES).and_then(|n| n.checked_add(HEADER));
        if expected != Some(bytes.len()) || pending && right == NO_PAGE {
            return Err(r.corrupt("its length or split link is invalid"));
        }
        let (words, partial) = r.rest().as_chunks::<WORD>();
        let (records, rest) = words.as_chunks::<ENTRY_WORDS>();
        if !partial.is_empty() || !rest.is_empty() {
            return Err(corrupt("its entries end inside a record"));
        }
        self.entries.clear();
        self.entries.reserve(count);
        for [min_x, min_y, max_x, max_y, id, value] in records {
            let [min_x, min_y, max_x, max_y] = [min_x, min_y, max_x, max_y].map(|w| f64::from_bits(u64::from_le_bytes(*w)));
            let bounds = Bounds::new([min_x, min_y], [max_x, max_y]).map_err(|_| corrupt("its bounds are invalid"))?;
            let (id, value) = (u64::from_le_bytes(*id), u64::from_le_bytes(*value));
            if level == 0 && value == 0 {
                return Err(corrupt("a leaf entry has no references"));
            }
            self.entries.push(Entry { bounds, id, value });
        }
        (self.level, self.stamp, self.pending) = (level, stamp, pending);
        self.retired = flags & RETIRED != 0;
        self.right = (right != NO_PAGE).then_some(PageId(right));
        Ok(())
    }

    /// The level a node's page records, from its header alone, with the
    /// header's own checks.
    pub(super) fn level_of(bytes: &[u8]) -> Result<u8> {
        header(&mut Reader::new(bytes, "R-link node")).map(|(level, ..)| level)
    }

    pub(super) fn bytes(&self) -> Result<Vec<u8>> {
        let count = u32::try_from(self.entries.len()).map_err(|_| corrupt("a node has too many entries"))?;
        let mut bytes = Vec::with_capacity(HEADER.saturating_add(self.entries.len().saturating_mul(ENTRY_BYTES)));
        bytes.put_u8(FORMAT);
        bytes.put_u8(self.level);
        bytes.put_u8(if self.pending { PENDING } else { 0 } | if self.retired { RETIRED } else { 0 });
        bytes.put_u64(self.stamp);
        bytes.put_u64(self.right.map_or(NO_PAGE, |p| p.0));
        bytes.put_u32(count);
        for e in &self.entries {
            for coordinate in e.bounds.lower().into_iter().chain(e.bounds.upper()) {
                bytes.put_u64(coordinate.to_bits());
            }
            bytes.put_u64(e.id);
            bytes.put_u64(e.value);
        }
        Ok(bytes)
    }

    pub(super) fn bounds(&self) -> Option<Bounds> {
        self.entries.iter().map(|e| e.bounds).reduce(Bounds::union)
    }
}

/// A node header's level, stamp, right link, pending flag and entry
/// count, with its format, level and flag checked.
fn header(r: &mut Reader<'_>) -> Result<(u8, u64, u64, u8, usize)> {
    let format = r.u8()?;
    let level = r.u8()?;
    let flags = r.u8()?;
    if format != FORMAT || level > MAX_LEVEL || flags > PENDING | RETIRED {
        return Err(r.corrupt("its format, level or flags are invalid"));
    }
    let stamp = r.u64()?;
    let right = r.u64()?;
    let count = usize::try_from(r.u32()?).map_err(|_| r.corrupt("its count does not fit"))?;
    Ok((level, stamp, right, flags, count))
}
