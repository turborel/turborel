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
/// Format, level, pending flag, stamp, right link and entry count.
pub(super) const HEADER: usize = 23;
/// Four coordinates followed by two u64 fields.
pub(super) const ENTRY_BYTES: usize = 48;
/// An absent sibling; the pager never allocates this page number.
const NO_PAGE: u64 = u64::MAX;
/// A corrupt tree cannot cause an unbounded descent or cursor stack.
pub(super) const MAX_LEVEL: u8 = 63;

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
    pub(super) entries: Vec<Entry>,
}

impl Node {
    pub(super) fn empty(id: PageId, level: u8) -> Node {
        Node { level, stamp: id.0, right: None, pending: false, entries: Vec::new() }
    }

    pub(super) fn parse(bytes: &[u8]) -> Result<Node> {
        let mut r = Reader::new(bytes, "R-link node");
        let format = r.u8()?;
        let level = r.u8()?;
        let pending = r.u8()?;
        if format != FORMAT || level > MAX_LEVEL || pending > 1 {
            return Err(r.corrupt("its format, level or pending flag is invalid"));
        }
        let stamp = r.u64()?;
        let right = r.u64()?;
        let count = usize::try_from(r.u32()?).map_err(|_| r.corrupt("its count does not fit"))?;
        let expected = count.checked_mul(ENTRY_BYTES).and_then(|n| n.checked_add(HEADER));
        if expected != Some(bytes.len()) || pending == 1 && right == NO_PAGE {
            return Err(r.corrupt("its length or split link is invalid"));
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let lower = [f64::from_bits(r.u64()?), f64::from_bits(r.u64()?)];
            let upper = [f64::from_bits(r.u64()?), f64::from_bits(r.u64()?)];
            let bounds = Bounds::new(lower, upper).map_err(|_| r.corrupt("its bounds are invalid"))?;
            let id = r.u64()?;
            let value = r.u64()?;
            if level == 0 && value == 0 {
                return Err(r.corrupt("a leaf entry has no references"));
            }
            entries.push(Entry { bounds, id, value });
        }
        r.finish()?;
        Ok(Node { level, stamp, right: (right != NO_PAGE).then_some(PageId(right)), pending: pending == 1, entries })
    }

    pub(super) fn bytes(&self) -> Result<Vec<u8>> {
        let count = u32::try_from(self.entries.len()).map_err(|_| corrupt("a node has too many entries"))?;
        let mut bytes = Vec::with_capacity(HEADER.saturating_add(self.entries.len().saturating_mul(ENTRY_BYTES)));
        bytes.put_u8(FORMAT);
        bytes.put_u8(self.level);
        bytes.put_u8(u8::from(self.pending));
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
