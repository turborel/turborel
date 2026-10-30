//! \[DESIGN\] A turn that work takes (RULES.md D55, R-ERR-4): threads take
//! it one at a time, waiting for each other; the thread holding it is
//! refused, so work that calls back into the database while it holds the
//! turn, as a plug-in may, fails rather than waits for itself. Split from
//! threads.rs (RULES.md R-SIZE-2).

use std::sync::{Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};

use crate::error::{Code, Result, SqlError};

/// A turn, and the thread holding it, if one is.
#[derive(Default)]
pub(crate) struct Turn {
    holder: Mutex<Option<ThreadId>>,
    ended: Condvar,
}

/// A thread's turn: dropping it ends the turn.
pub(crate) struct Turned<'a> {
    turn: &'a Turn,
}

impl Turn {
    /// The calling thread's turn, once no other holds it; `what` names the
    /// work, for the error.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the calling thread holds it already: it was called
    /// back from within its own work.
    pub(crate) fn take(&self, what: &str) -> Result<Turned<'_>> {
        let me = thread::current().id();
        // The slot is only ever set and cleared, so a lock poisoned by a
        // panicking thread still guards a valid slot.
        let mut holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match *holder {
                None => break,
                Some(other) => {
                    if other == me {
                        return Err(SqlError::new(
                            Code::Api,
                            format!("[DESIGN] {what} was asked for from within itself, as by a plug-in calling back (RULES.md R-ERR-4)"),
                        ));
                    }
                    holder = self.ended.wait(holder).unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
        *holder = Some(me);
        Ok(Turned { turn: self })
    }
}

impl Drop for Turned<'_> {
    fn drop(&mut self) {
        *self.turn.holder.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.turn.ended.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// Verifies: [DESIGN] RULES.md D55, R-ERR-4: a turn's holder taking it
    /// again is refused (SQLCODE -910); another thread waits for it, and
    /// takes it once it ends.
    #[test]
    fn a_turn_refuses_its_holder() {
        let turn = Arc::new(Turn::default());
        let held = turn.take("the work").unwrap();
        assert_eq!(turn.take("the work").err().map(|e| e.code), Some(Code::Api));
        let other = Arc::clone(&turn);
        let waiting = thread::spawn(move || other.take("the work").is_ok());
        drop(held);
        assert!(waiting.join().unwrap());
        assert!(turn.take("the work").is_ok(), "ended again");
    }
}
