//! \[DESIGN\] What the label and reachability index methods give a path
//! search (RULES.md D115, D116), as `adjacency.rs` is what the adjacency
//! method gives: the members of an element table by dense id, and whether
//! vertices reach others by an edge table's edges.

use std::collections::HashMap;

use crate::error::Result;
use crate::index::Key;
use crate::storage::RowId;

/// Bits of a word of a bitmap.
pub const WORD_BITS: usize = 64;

/// \[DESIGN\] The rows of an element table a label index holds, each
/// numbered densely: its rows by number and a bitmap of the numbers in use.
/// A row may be of a version a reader does not see (RULES.md D34).
#[derive(Debug, Clone, Default)]
pub struct MemberSet {
    /// Each number's row.
    rows: Vec<RowId>,
    /// Of each number, whether a row of it has an entry, by bit.
    live: Vec<u64>,
    /// Each row's number.
    ids: HashMap<RowId, u32>,
}

/// The word and bit of number `id` in a bitmap.
pub fn place(id: usize) -> (usize, u64) {
    let bit = id.checked_rem(WORD_BITS).and_then(|b| u32::try_from(b).ok()).unwrap_or_default();
    (id.checked_div(WORD_BITS).unwrap_or_default(), 1_u64.checked_shl(bit).unwrap_or_default())
}

impl MemberSet {
    /// The set of `rows` by number, those of `live` numbers in use.
    pub fn new(rows: Vec<RowId>, live: &[bool]) -> MemberSet {
        let mut bits = vec![0_u64; rows.len().div_ceil(WORD_BITS)];
        for (id, _) in live.iter().enumerate().filter(|(_, l)| **l) {
            let (word, bit) = place(id);
            if let Some(w) = bits.get_mut(word) {
                *w |= bit;
            }
        }
        let ids = rows.iter().zip(0_u32..).map(|(&r, id)| (r, id)).collect();
        MemberSet { rows, live: bits, ids }
    }

    /// The numbers given, in use or not.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether it numbers no row.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The number of `row`, if it has one in use.
    pub fn id(&self, row: RowId) -> Option<u32> {
        self.ids.get(&row).copied().filter(|&id| self.holds(id))
    }

    /// Whether number `id` is in use.
    pub fn holds(&self, id: u32) -> bool {
        let (word, bit) = place(usize::try_from(id).unwrap_or(usize::MAX));
        self.live.get(word).is_some_and(|w| w & bit != 0)
    }

    /// The bitmap of the numbers in use, a word for each 64.
    pub fn bitmap(&self) -> &[u64] {
        &self.live
    }

    /// The row of number `id`.
    pub fn row(&self, id: u32) -> Option<RowId> {
        self.rows.get(usize::try_from(id).ok()?).copied()
    }
}

/// \[DESIGN\] An element table's members, as a label index keeps them.
pub trait Members: Send + Sync {
    /// The rows it holds now, numbered, if it can give them: none if its
    /// memory budget refused them, and the table is then read instead.
    ///
    /// # Errors
    ///
    /// The index's storage fails.
    fn members(&self) -> Result<Option<std::sync::Arc<MemberSet>>>;
}

/// \[DESIGN\] Whether vertices reach others by the edges of one edge
/// table, as a reachability index keeps them. The answer may say a vertex
/// reaches one it does not, never the reverse, so a search pruned by it
/// finds what it would find unpruned.
pub trait Reachability: Send + Sync {
    /// Of each of `starts`, whether it reaches one of `ends` by following
    /// edges from source to destination, or, if `reverse`, from
    /// destination to source, in zero or more steps: each vertex named by
    /// its key, of the index's source columns' types. None if the index
    /// cannot answer now, as after a delete until its maintenance.
    ///
    /// # Errors
    ///
    /// The index's storage fails.
    fn reaching(&self, starts: &[Key], ends: &[Key], reverse: bool) -> Result<Option<Vec<bool>>>;
}
