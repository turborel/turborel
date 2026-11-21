//! One thread's turn to change the definitions (`Database::take_turn`).

use std::sync::PoisonError;

use super::Shared;

/// One thread's turn to change the definitions. Dropping it ends the turn.
pub(super) struct Turn<'a> {
    pub(super) shared: &'a Shared,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        // The slot is only ever set and cleared, so a lock poisoned by a
        // panicking thread still guards a valid slot.
        let mut changing = self.shared.changing.lock().unwrap_or_else(PoisonError::into_inner);
        *changing = None;
        self.shared.turn_ended.notify_one();
    }
}
