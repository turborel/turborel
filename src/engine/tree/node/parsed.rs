//! \[DESIGN\] A node's header as parsed, kept apart from its bytes (RULES.md
//! D808). A read that keeps a copy of a leaf parses it once, and each later
//! read of the copy rebuilds the node from these fields with no header read
//! and no check made again.

use super::{HEADER, Node, PageId};

/// The fields `Node::parse` reads from a node's header, its keys' prefix
/// and high key as places in the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Parsed {
    level: u8,
    high: Option<(usize, usize)>,
    right: Option<PageId>,
    deleted: bool,
    tree: u64,
    prefix: (usize, usize),
    first: PageId,
    count: usize,
    garbage: usize,
    last: Option<usize>,
    heap: usize,
    slots: usize,
}

impl<'a> Node<'a> {
    /// Its header as parsed, for `parsed_again` over the same bytes.
    pub(crate) fn parsed(&self) -> Parsed {
        Parsed {
            level: self.level,
            // The high key follows the header, and the prefix ends where
            // the heap begins (`Node::parse`).
            high: self.high.map(|high| (HEADER, high.len())),
            right: self.right,
            deleted: self.deleted,
            tree: self.tree,
            prefix: (self.heap.saturating_sub(self.prefix.len()), self.prefix.len()),
            first: self.first,
            count: self.count,
            garbage: self.garbage,
            last: self.last,
            heap: self.heap,
            slots: self.slots,
        }
    }

    /// The node `bytes` hold, which `parsed` was taken of; none if they
    /// are too short for it, so are not those bytes.
    pub(crate) fn parsed_again(bytes: &'a [u8], parsed: &Parsed) -> Option<Node<'a>> {
        let part = |(at, len): (usize, usize)| bytes.get(at..at.checked_add(len)?);
        Some(Node {
            bytes,
            level: parsed.level,
            high: match parsed.high {
                Some(high) => Some(part(high)?),
                None => None,
            },
            right: parsed.right,
            deleted: parsed.deleted,
            tree: parsed.tree,
            prefix: part(parsed.prefix)?,
            first: parsed.first,
            count: parsed.count,
            garbage: parsed.garbage,
            last: parsed.last,
            heap: parsed.heap,
            slots: parsed.slots,
        })
    }
}
