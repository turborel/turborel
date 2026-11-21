//! \[DESIGN\] Where a row's entry was last seen, handed from one tree to
//! another on one thread (RULES.md D500). A table's tree notes the leaf it
//! wrote a row into, and a scan of it the leaf it read one from; an index
//! entry written next for that row keeps the leaf as its hint. An index
//! read notes each entry's hint, and the table's read of that row tries
//! the leaf first (`near.rs`).
//!
//! A statement writes a row and then its index entries, and reads an index
//! entry and then its row, on one thread, so the note is read just after it
//! is made, and a few kept per thread are enough. A note is only a guess:
//! of another table's row of the same id, or of a leaf since split or
//! freed, it costs one page looked at, never a wrong answer, as a leaf is
//! checked before it is trusted.

use std::cell::RefCell;

/// \[DESIGN\] The notes kept per thread: the rows read or written by one
/// statement step, with room for the reads a scan runs ahead.
const KEPT: usize = 16;
/// No leaf: page 0 is the pager's own, never a node's.
pub(crate) const NO_LEAF: u64 = 0;

/// A thread's notes, the newest at `next - 1`.
struct Notes {
    rows: [u64; KEPT],
    leaves: [u64; KEPT],
    next: usize,
}

std::thread_local! {
    static NOTES: RefCell<Notes> = const { RefCell::new(Notes { rows: [0; KEPT], leaves: [NO_LEAF; KEPT], next: 0 }) };
}

/// Notes that row `row`'s entry is in leaf `leaf`.
pub(crate) fn note(row: u64, leaf: u64) {
    NOTES.with(|notes| {
        // Taken already only by a note of this thread, which never calls
        // back in: none is then kept, which costs a guess.
        let Ok(mut held) = notes.try_borrow_mut() else { return };
        let Notes { rows, leaves, next } = &mut *held;
        if let (Some(r), Some(l)) = (rows.get_mut(*next), leaves.get_mut(*next)) {
            (*r, *l) = (row, leaf);
        }
        *next = next.saturating_add(1).checked_rem(KEPT).unwrap_or_default();
    });
}

/// The leaf last noted for row `row` on this thread, if one was.
pub(crate) fn leaf_of(row: u64) -> Option<u64> {
    NOTES
        .with(|notes| {
            let notes = notes.try_borrow().ok()?;
            // Newest first, so a row noted again is found at its latest leaf.
            (1..=KEPT).find_map(|back| {
                let at = notes.next.checked_add(KEPT)?.checked_sub(back)?.checked_rem(KEPT)?;
                (notes.rows.get(at) == Some(&row)).then(|| notes.leaves.get(at).copied()).flatten()
            })
        })
        .filter(|&leaf| leaf != NO_LEAF)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D500: a row's leaf noted is found,
    /// the latest of two notes of one row first; a row not noted, or
    /// noted with no leaf, or pushed out by `KEPT` notes after it, has
    /// none.
    #[test]
    fn notes_keep_the_latest_leaf_of_a_row() {
        note(7, 70);
        note(8, 80);
        note(7, 71);
        assert_eq!((leaf_of(7), leaf_of(8), leaf_of(9)), (Some(71), Some(80), None));
        note(9, NO_LEAF);
        assert_eq!(leaf_of(9), None);
        for row in 100..100 + u64::try_from(KEPT).unwrap() {
            note(row, row);
        }
        assert_eq!((leaf_of(7), leaf_of(100)), (None, Some(100)));
    }
}
