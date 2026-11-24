//! \[DESIGN\] What a debugger and its target share (RULES.md D1005 to
//! D1007, D1010, D1011): the breakpoints, how the target is to go on, the
//! frames of the programs it runs, its pause, and the events the debugger
//! has not read yet, behind one lock, with a condition variable each side
//! waits on.
//!
//! The target's thread alone pushes and pops frames and pauses. The
//! debugger's sets and clears breakpoints, gives its word to a pause,
//! waits for events and detaches. A frame is pushed only by the thread
//! whose frames the stack holds, while it holds any: a function a query
//! evaluates on a thread of its own runs with no frame and no pause, so
//! the stack stays one thread's calls (RULES.md D1012).

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use super::{Action, Breakpoint, Event, Frame, PAUSE_POLL, Paused, Reason};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::sync::{lock, lock_builtin};

/// How the target goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// To a breakpoint.
    Run,
    /// To the next statement of any frame.
    Into,
    /// To the next statement of a frame this deep or less.
    Over(usize),
    /// To the next statement of a frame less deep than this.
    Out(usize),
}

/// What the two sides share.
#[derive(Debug)]
struct Shared {
    /// The user the debugger runs as, whose privileges say which frames
    /// it sees into (RULES.md D1008).
    debugger: String,
    /// The debugger detached.
    detached: bool,
    /// The target session ended.
    ended: bool,
    breakpoints: Vec<Breakpoint>,
    numbered: u32,
    mode: Mode,
    /// The frames of the programs the target runs, the outermost first.
    frames: Vec<Frame>,
    /// The thread whose frames they are, while there are any.
    owner: Option<ThreadId>,
    paused: Option<Paused>,
    /// The debugger's word to the pause.
    word: Option<Action>,
    events: VecDeque<Event>,
}

/// \[DESIGN\] What a debugger and its target share.
#[derive(Debug)]
pub struct Link {
    shared: Mutex<Shared>,
    changed: Condvar,
}

/// The refusal of a debugger's request its target's state does not allow
/// (SQLSTATE TBD01, RULES.md D1013).
pub(crate) fn refused(msg: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] {msg}")).with_state(SqlState::DEBUGGER)
}

impl Link {
    /// A link of a debugger running as `debugger`, its target running.
    pub(crate) fn new(debugger: &str) -> Link {
        let shared = Shared {
            debugger: debugger.to_owned(),
            detached: false,
            ended: false,
            breakpoints: Vec::new(),
            numbered: 0,
            mode: Mode::Run,
            frames: Vec::new(),
            owner: None,
            paused: None,
            word: None,
            events: VecDeque::new(),
        };
        Link { shared: Mutex::new(shared), changed: Condvar::new() }
    }

    fn shared(&self) -> Result<MutexGuard<'_, Shared>> {
        lock(&self.shared)
    }

    /// The user the debugger runs as.
    pub(crate) fn debugger(&self) -> Result<String> {
        Ok(self.shared()?.debugger.clone())
    }

    // ---------------------------------------------------------------------
    // The target's side
    // ---------------------------------------------------------------------

    /// `frame` pushed, its depth's index given: none if the stack holds
    /// another thread's frames, or the debugger has gone.
    pub(crate) fn push(&self, frame: Frame) -> Option<usize> {
        let mut shared = lock_builtin(&self.shared);
        let me = std::thread::current().id();
        if shared.detached || shared.owner.is_some_and(|owner| owner != me) {
            return None;
        }
        shared.owner = Some(me);
        shared.frames.push(frame);
        Some(shared.frames.len().saturating_sub(1))
    }

    /// The frame `index` and those it called popped, as its program ends.
    pub(crate) fn pop(&self, index: usize) {
        let mut shared = lock_builtin(&self.shared);
        shared.frames.truncate(index);
        if shared.frames.is_empty() {
            shared.owner = None;
        }
    }

    /// The frame `index` as `f` leaves it.
    pub(crate) fn update(&self, index: usize, f: impl FnOnce(&mut Frame)) {
        if let Some(frame) = lock_builtin(&self.shared).frames.get_mut(index) {
            f(frame);
        }
    }

    /// Why the statement of the frame `index`, of the routine or trigger
    /// `name`, beginning at `at`, pauses, if it does: by the step under way,
    /// else by a breakpoint there (RULES.md D1007).
    pub(crate) fn stops(&self, index: usize, name: Option<&QualifiedName>, at: usize) -> Option<Reason> {
        let shared = lock_builtin(&self.shared);
        let depth = index.saturating_add(1);
        let stepped = match shared.mode {
            Mode::Run => false,
            Mode::Into => true,
            Mode::Over(d) => depth <= d,
            Mode::Out(d) => depth < d,
        };
        if stepped {
            return Some(Reason::Step);
        }
        let name = name?;
        let found = shared.breakpoints.iter().find(|b| b.routine == *name && b.span.0 <= at && at < b.span.1);
        found.map(|b| Reason::Breakpoint(b.number))
    }

    /// The target paused for `reason` in the frame `index`: the debugger
    /// told, and its word waited for, at most `most`, or until `interrupted`
    /// says the statement is cancelled. Then it goes on as told, else to a
    /// breakpoint (RULES.md D1010, D1011).
    ///
    /// # Errors
    ///
    /// The cancellation's; SQLCODE -910 if the link's lock is poisoned.
    pub(crate) fn pause(&self, (index, reason): (usize, Reason), most: Duration, interrupted: &dyn Fn() -> Result<()>) -> Result<()> {
        let began = Instant::now();
        let mut shared = self.shared()?;
        if shared.detached {
            return Ok(());
        }
        let paused = Paused { reason, frames: shared.frames.clone() };
        shared.paused = Some(paused.clone());
        shared.word = None;
        shared.events.push_back(Event::Paused(paused));
        self.changed.notify_all();
        loop {
            if shared.detached {
                resumed(&mut shared, Mode::Run);
                return Ok(());
            }
            if let Some(word) = shared.word.take() {
                let depth = index.saturating_add(1);
                let mode = match word {
                    Action::Into => Mode::Into,
                    Action::Over => Mode::Over(depth),
                    Action::Out => Mode::Out(depth),
                    Action::Continue => Mode::Run,
                };
                resumed(&mut shared, mode);
                return Ok(());
            }
            if began.elapsed() >= most {
                resumed(&mut shared, Mode::Run);
                shared.events.push_back(Event::TimedOut);
                self.changed.notify_all();
                return Ok(());
            }
            drop(shared);
            if let Err(e) = interrupted() {
                let mut again = self.shared()?;
                resumed(&mut again, Mode::Run);
                again.events.push_back(Event::Cancelled);
                self.changed.notify_all();
                return Err(e);
            }
            shared = self.shared()?;
            shared = self.changed.wait_timeout(shared, PAUSE_POLL).map_err(|_| poisoned())?.0;
        }
    }

    /// The target session ended.
    pub(crate) fn end(&self) {
        let mut shared = lock_builtin(&self.shared);
        shared.ended = true;
        shared.events.push_back(Event::Ended);
        self.changed.notify_all();
    }

    // ---------------------------------------------------------------------
    // The debugger's side
    // ---------------------------------------------------------------------

    /// A breakpoint of `routine` at `line`, of the characters `span`, on
    /// `condition` if given: its number.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the link's lock is poisoned.
    pub(crate) fn break_at(&self, routine: QualifiedName, (line, span): (u32, (usize, usize)), condition: Option<String>) -> Result<u32> {
        let mut shared = self.shared()?;
        shared.numbered = shared.numbered.saturating_add(1);
        let number = shared.numbered;
        shared.breakpoints.push(Breakpoint { number, routine, line, span, condition });
        Ok(number)
    }

    /// The breakpoint `number` cleared.
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if there is none of that number.
    pub(crate) fn clear(&self, number: u32) -> Result<()> {
        let mut shared = self.shared()?;
        let before = shared.breakpoints.len();
        shared.breakpoints.retain(|b| b.number != number);
        if shared.breakpoints.len() == before {
            return Err(refused(&format!("the debugger has no breakpoint {number}")));
        }
        Ok(())
    }

    /// The breakpoints, in the order they were set.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the link's lock is poisoned.
    pub(crate) fn breakpoints(&self) -> Result<Vec<Breakpoint>> {
        Ok(self.shared()?.breakpoints.clone())
    }

    /// The paused target told to go on as `word` says.
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if it is not paused, or has been told already.
    pub(crate) fn resume(&self, word: Action) -> Result<()> {
        let mut shared = self.shared()?;
        if shared.paused.is_none() || shared.word.is_some() {
            return Err(refused("the debugged session is not paused"));
        }
        shared.word = Some(word);
        self.changed.notify_all();
        Ok(())
    }

    /// The running target told to pause at its next statement.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the link's lock is poisoned.
    pub(crate) fn interrupt(&self) -> Result<()> {
        let mut shared = self.shared()?;
        if shared.paused.is_none() {
            shared.mode = Mode::Into;
        }
        Ok(())
    }

    /// The target's pause, if it is paused.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the link's lock is poisoned.
    pub(crate) fn paused(&self) -> Result<Option<Paused>> {
        Ok(self.shared()?.paused.clone())
    }

    /// The next event the debugger has not read: waited for at most
    /// `most`; none if none came.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the link's lock is poisoned.
    pub(crate) fn next_event(&self, most: Duration) -> Result<Option<Event>> {
        let began = Instant::now();
        let mut shared = self.shared()?;
        loop {
            if let Some(event) = shared.events.pop_front() {
                return Ok(Some(event));
            }
            let left = most.saturating_sub(began.elapsed());
            if left.is_zero() || shared.ended {
                return Ok(None);
            }
            shared = self.changed.wait_timeout(shared, left).map_err(|_| poisoned())?.0;
        }
    }

    /// Whether the target session has ended.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the link's lock is poisoned.
    pub(crate) fn ended(&self) -> Result<bool> {
        Ok(self.shared()?.ended)
    }

    /// The debugger detached: its breakpoints gone, the target, if paused,
    /// going on, and no frame pushed from now on.
    pub(crate) fn detach(&self) {
        let mut shared = lock_builtin(&self.shared);
        shared.detached = true;
        shared.breakpoints.clear();
        shared.mode = Mode::Run;
        self.changed.notify_all();
    }
}

/// The pause of `shared` over, the target going on in `mode`.
fn resumed(shared: &mut Shared, mode: Mode) {
    shared.paused = None;
    shared.word = None;
    shared.mode = mode;
}

/// A poisoned lock of a link, reported (RULES.md R-ERR-4).
fn poisoned() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a debugger's link was poisoned")
}
