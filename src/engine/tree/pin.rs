//! \[DESIGN\] (RULES.md D857) A tree pinned by a thread for a statement.
//! Every operation on a tree is counted in its epoch as it begins and
//! uncounted as it ends (`reclaim.rs`), two writes of a counter in one
//! total order, and a statement makes many operations on each tree it
//! uses. A thread that pins a tree is counted once, for as long as the pin
//! lives, and the operations it makes on that tree meanwhile count
//! nothing: the pin's count already keeps every page they may hold.
//!
//! A pin is the epoch count an operation takes, held longer, so whatever
//! holds of an operation under way holds of everything the thread does
//! while pinned. A page taken out is freed only once no count of the
//! epoch it was taken out in, or of the one before, is left, and the
//! pin's is one. A longer count only delays the freeing of pages taken
//! out, as a long scan's does.
//!
//! The thread's pins are noted in a few places it alone reads. A pin
//! cannot leave its thread, so the note is true of the thread that reads
//! it. A thread with every place taken pins as before, and its operations
//! count themselves.

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

use super::BLinkTree;
use super::reclaim::{Entered, Epochs, Guard};

/// \[DESIGN\] The trees a thread notes as pinned at once: a transaction's
/// tables and their indexes. More are pinned unnoted.
const NOTED: usize = 16;

thread_local! {
    /// The numbers of the epochs this thread has pinned and noted; 0 for
    /// a place free.
    static PINNED: Cell<[u64; NOTED]> = const { Cell::new([0; NOTED]) };
}

/// \[DESIGN\] A tree pinned by the thread that made it, until it is
/// dropped (`BLinkTree::pin`). It cannot be sent to another thread.
pub struct Pinned<'t> {
    epochs: &'t Epochs,
    entered: Entered,
    /// The place it is noted in, if one was free.
    place: Option<usize>,
    /// Keeps it on its thread, whose note of it is no other's.
    _here: PhantomData<Rc<()>>,
}

impl Drop for Pinned<'_> {
    fn drop(&mut self) {
        // Unnoted before it is uncounted, so no operation of the thread
        // counts on a pin gone.
        if let Some(place) = self.place {
            PINNED.with(|pinned| {
                let mut noted = pinned.get();
                if let Some(slot) = noted.get_mut(place) {
                    *slot = 0;
                }
                pinned.set(noted);
            });
        }
        self.epochs.leave(self.entered);
    }
}

impl Epochs {
    /// Counts the thread in the epoch current until the pin is dropped,
    /// and notes it, so its operations count nothing meanwhile.
    pub(super) fn pin(&self) -> Pinned<'_> {
        let entered = self.enter();
        let number = self.number();
        let place = PINNED.with(|pinned| {
            let mut noted = pinned.get();
            // Noted already, by a pin still held, it is not noted twice:
            // the first pin dropped would unnote the second.
            let place = if noted.contains(&number) { None } else { noted.iter().position(|&slot| slot == 0) };
            if let Some(slot) = place.and_then(|place| noted.get_mut(place)) {
                *slot = number;
                pinned.set(noted);
            }
            place
        });
        Pinned { epochs: self, entered, place, _here: PhantomData }
    }

    /// An operation guarded until the guard is dropped; none if the
    /// thread has the tree pinned, which guards it already.
    pub(super) fn guard(&self) -> Option<Guard<'_>> {
        let number = self.number();
        if PINNED.with(|pinned| pinned.get().contains(&number)) {
            return None;
        }
        Some(self.guarded())
    }
}

impl BLinkTree {
    /// \[DESIGN\] Pins the tree for the calling thread until the pin is
    /// dropped (RULES.md D857): the thread's reads and writes of the tree
    /// meanwhile enter no epoch of their own. For a statement, or a
    /// transaction's step, that makes many of them. The pin delays the
    /// freeing of pages taken out of the tree, so it is not held while
    /// the thread waits for a client.
    #[must_use]
    pub fn pin(&self) -> Pinned<'_> {
        self.epochs.pin()
    }
}
