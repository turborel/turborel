//! \[DESIGN\] A turn that work takes (RULES.md D55, R-ERR-4): threads take
//! it one at a time, waiting for each other; the thread holding it is
//! refused, so work that calls back into the database while it holds the
//! turn, as a plug-in may, fails rather than waits for itself. Work may
//! also hold the turn shared, several at once, as the exports of a
//! database hold the checkpoints' turn for their lives (RULES.md D57):
//! a shared hold waits for the thread taking the turn, and the turn is
//! taken once every shared hold has left. Split from threads.rs (RULES.md
//! R-SIZE-2).

use std::sync::{Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};

use crate::error::{Code, Result, SqlError};

/// A turn: the thread holding it, if one does, and the threads holding
/// it shared, each hold once.
#[derive(Default)]
pub(crate) struct Turn {
    holder: Mutex<Holding>,
    ended: Condvar,
}

/// Who holds a turn.
#[derive(Default)]
struct Holding {
    taken: Option<ThreadId>,
    shared: Vec<ThreadId>,
}

/// A shared hold of a turn: the thread that took it, by which it leaves.
pub(crate) struct Hold(ThreadId);

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
        // The slots are only ever set and cleared, so a lock poisoned by a
        // panicking thread still guards valid slots.
        let mut holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if holder.taken == Some(me) || holder.shared.contains(&me) {
                return Err(within(what));
            }
            if holder.taken.is_none() && holder.shared.is_empty() {
                break;
            }
            holder = self.ended.wait(holder).unwrap_or_else(PoisonError::into_inner);
        }
        holder.taken = Some(me);
        Ok(Turned { turn: self })
    }
}

/// The error of a thread asking for a turn it holds (SQLCODE -910).
fn within(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {what} was asked for from within itself, as by a plug-in calling back (RULES.md R-ERR-4)"))
}

impl Turn {
    /// \[DESIGN\] Holds the turn shared for the calling thread, once no
    /// thread has taken it, with no guard: held until `leave`, by whatever
    /// owns the hold, as an export of a database holds the checkpoints'
    /// turn for its life (RULES.md D57), read from any thread; other holds
    /// are not waited for.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the calling thread has taken the turn.
    pub(crate) fn hold(&self, what: &str) -> Result<Hold> {
        let me = thread::current().id();
        let mut holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match holder.taken {
                None => break,
                Some(other) => {
                    if other == me {
                        return Err(within(what));
                    }
                    holder = self.ended.wait(holder).unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
        holder.shared.push(me);
        Ok(Hold(me))
    }

    /// Ends the hold `hold` took, from any thread.
    pub(crate) fn leave(&self, Hold(left): Hold) {
        let mut holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(index) = holder.shared.iter().position(|&held| held == left) {
            holder.shared.swap_remove(index);
        }
        self.ended.notify_all();
    }
}

impl Drop for Turned<'_> {
    fn drop(&mut self) {
        self.turn.holder.lock().unwrap_or_else(PoisonError::into_inner).taken = None;
        self.turn.ended.notify_all();
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

    /// Verifies: [DESIGN] RULES.md D55, D57, R-ERR-4: a turn held shared is
    /// held again at once, by the same thread too; taking it on a holding
    /// thread is refused (SQLCODE -910), and on another waits until every
    /// hold has left; a hold waits for the thread that took the turn, and
    /// is refused that thread.
    #[test]
    fn a_turn_is_held_shared() {
        let turn = Arc::new(Turn::default());
        let (one, two) = (turn.hold("an export").unwrap(), turn.hold("an export").unwrap());
        assert_eq!(turn.take("the work").err().map(|e| e.code), Some(Code::Api), "held here");
        let other = Arc::clone(&turn);
        let waiting = thread::spawn(move || other.take("the work").map(|_| ()).map_err(|e| e.code));
        turn.leave(one);
        thread::sleep(std::time::Duration::from_millis(20));
        assert!(!waiting.is_finished(), "one hold left: the other keeps the turn");
        turn.leave(two);
        assert_eq!(waiting.join().unwrap(), Ok(()));
        let taken = turn.take("the work").unwrap();
        assert_eq!(turn.hold("an export").err().map(|e| e.code), Some(Code::Api), "taken here");
        let another = Arc::clone(&turn);
        let holding = thread::spawn(move || another.hold("an export").map(|hold| another.leave(hold)).map_err(|e| e.code));
        drop(taken);
        assert_eq!(holding.join().unwrap(), Ok(()));
    }
}
