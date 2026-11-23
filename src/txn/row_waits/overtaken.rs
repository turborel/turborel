//! \[DESIGN\] A waiter overtaken at its turn (RULES.md D847). Once a row's
//! writer ends, the row is not kept for its first waiter while that waiter
//! is still to be woken and run: a transaction that is running and comes
//! to the row writes it at once. The first waiter, waking to find that
//! writer, queues first in the new writer's line, the waiters behind it
//! for the row moved there with it in their order and unwoken, and claims
//! the row firmly. A firm claim holds back every newcomer as D456's claim
//! did, so a waiter is overtaken at most once.

use std::sync::Arc;

use super::{Claim, Turn, deadlock, move_line};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::storage::RowId;
use crate::sync::{lock, lock_builtin};
use crate::txn::{TxId, Txn};

impl Txn {
    /// \[DESIGN\] RULES.md D847: in its `turn` for row `id` of `table`,
    /// this transaction found `writer` writing the row ahead of it: it
    /// queues first in `writer`'s line, the waiters behind it for the row
    /// with it, its claim of the row firm, and waits for its turn there.
    /// None if `writer` has ended meanwhile, to look at the row again.
    ///
    /// # Errors
    ///
    /// As `wait_for_row`'s.
    pub(crate) fn overtaken(&self, turn: Turn, (table, id): (&QualifiedName, RowId), writer: TxId) -> Result<Option<Turn>> {
        let Some(next) = self.origin.shared.ends.of(writer) else { return Ok(None) };
        let seat = Arc::clone(&turn.seat);
        {
            let mut waits_for = lock(&self.origin.shared.waits_for)?;
            if super::super::locks::would_deadlock(&waits_for, self.id, &[writer]) {
                return Err(deadlock(table));
            }
            let mut from = lock_builtin(&turn.ending.queue);
            let mut to = lock_builtin(&next.queue);
            if to.ended {
                return Ok(None);
            }
            let ticket = to.ticket();
            {
                let mut place = lock_builtin(&seat.place);
                place.ending = Arc::clone(&next);
                place.ticket = ticket;
                place.turn = false;
                place.lost = true;
            }
            to.seated.push((ticket, Arc::clone(&seat)));
            waits_for.insert(self.id, vec![writer]);
            move_line((&mut from, (&mut to, &next)), (table, id), writer, &mut waits_for);
            let claim = Claim { owner: writer, line: Arc::clone(&next), first: self.id, firm: true };
            self.origin.shared.ends.claims.set((table, id), Some(claim));
        }
        // The turn in the old line passes on, the seat no longer there.
        drop(turn);
        self.shown_waiting((table, writer), |began| self.sleep_on(lock_builtin(&seat.place), &seat, began)).map(Some)
    }
}
