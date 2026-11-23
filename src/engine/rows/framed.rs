//! \[DESIGN\] The buffer a row's record is encoded into, kept by its thread
//! (RULES.md D875). A write encodes its row behind the room for its
//! version's header (D846), gives the bytes to the tree, which copies
//! them into the leaf, and has no more use for them. The buffer goes back
//! to its thread as the write lets it go, and the thread's next write
//! encodes into it, so a row written allocates no buffer for its record.

use std::cell::Cell;
use std::ops::{Deref, DerefMut};

/// \[DESIGN\] The longest buffer a thread keeps: a record in line is
/// shorter than a page, and one longer, sealed or of many long values, is
/// not held for the writes after it.
const MOST_KEPT: usize = 1 << 16;

thread_local! {
    /// The buffer this thread's last write let go, empty if a write holds it.
    static KEPT: Cell<Vec<u8>> = const { Cell::new(Vec::new()) };
}

/// \[DESIGN\] A record encoded behind the room for its version's header,
/// in a buffer given back to its thread as it is dropped.
pub(super) struct Framed(Vec<u8>);

impl Framed {
    /// The buffer this thread keeps, empty, or a new one if a write holds
    /// it or the thread is ending.
    pub(super) fn buffer() -> Vec<u8> {
        let mut kept = KEPT.try_with(Cell::take).unwrap_or_default();
        kept.clear();
        kept
    }

    /// `bytes`, to go back to the thread once let go.
    pub(super) fn of(bytes: Vec<u8>) -> Framed {
        Framed(bytes)
    }
}

impl Deref for Framed {
    type Target = Vec<u8>;

    fn deref(&self) -> &Vec<u8> {
        &self.0
    }
}

impl DerefMut for Framed {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}

impl Drop for Framed {
    fn drop(&mut self) {
        if self.0.capacity() <= MOST_KEPT {
            // A thread ending keeps nothing more.
            let _ = KEPT.try_with(|kept| kept.set(std::mem::take(&mut self.0)));
        }
    }
}

// Unit tests: the buffer goes back to its thread (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::{Framed, MOST_KEPT};

    /// Verifies: [DESIGN] RULES.md D875: a record's buffer let go is the
    /// one the thread's next write encodes into, emptied; two held at once
    /// are two buffers; and one longer than a thread keeps is not kept.
    #[test]
    fn a_buffer_let_go_is_the_next_writes() {
        let mut first = Framed::of(Framed::buffer());
        first.extend_from_slice(&[7; 100]);
        let at = first.as_ptr();
        let second = Framed::of(Framed::buffer());
        assert_eq!(second.capacity(), 0, "the thread's is held");
        drop(second);
        drop(first);
        let again = Framed::of(Framed::buffer());
        assert_eq!((again.as_ptr(), again.len()), (at, 0), "the same buffer, emptied");
        drop(again);
        let mut long = Framed::of(Framed::buffer());
        long.reserve(MOST_KEPT + 1);
        drop(long);
        assert_eq!(Framed::buffer().capacity(), 0, "not kept");
    }
}
