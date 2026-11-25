//! \[EXTENSION\] The routine debugger, from a client (RULES.md D1030 to
//! D1035): a connection granted `debugger` attaches to the session of
//! another connection, by that one's ID, sets breakpoints in the routines
//! and triggers its user may debug, and drives that session's routines as
//! they pause, step by step, reading their call stack, parameters,
//! variables and cursors. What it hears of its target, an `Event`, comes
//! unasked, and is kept until `debug_wait` reads it.
//!
//! Two connections: the target runs its statements as ever; the debugger
//! waits for its events beside it, often on another thread.

use std::io::{BufRead, ErrorKind, Read, Write};
use std::time::Duration;

use turborel::catalog::QualifiedName;
use turborel_protocol::capability::DEBUGGER;
use turborel_protocol::{DebugReply, DebugRequest, Reply, Request};

pub use turborel::debugger::{Action, CursorShown, Event, Frame, Kind, Named, Origin, Paused, Reason};
pub use turborel_protocol::DebugBreakpoint;

use crate::connection::{Connection, Result};
use crate::frames::Answer;
use crate::timeouts::{io_failed, uninterrupted};

impl<S: Read + Write> Connection<S> {
    /// The reply to the Debug request `request`, but a completion.
    fn debugged(&mut self, request: DebugRequest) -> Result<DebugReply> {
        self.needs(false, DEBUGGER)?;
        self.send(&Request::Debug(request))?;
        match self.receive()? {
            Reply::Debug(reply) => Ok(reply),
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Complete(_)
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong
            | Reply::Event(_) => Err(self.lose("[DESIGN] the server sent no Debug reply")),
        }
    }

    /// The Debug request `request` sent, its completion read.
    fn debug_done(&mut self, request: DebugRequest) -> Result<()> {
        self.needs(false, DEBUGGER)?;
        self.completed(&Request::Debug(request)).map(drop)
    }

    /// \[EXTENSION\] This connection's ID, as INFORMATION_SCHEMA.CONNECTIONS
    /// shows it: what a debugger attaches to its session by.
    ///
    /// # Errors
    ///
    /// A connection failure: `debugger` not granted, or the server gone.
    pub fn debug_id(&mut self) -> Result<u64> {
        match self.debugged(DebugRequest::Identify)? {
            DebugReply::Connection(id) => Ok(id),
            DebugReply::Breakpoint(_) | DebugReply::Breakpoints(_) | DebugReply::Stack(_) => {
                Err(self.lose("[DESIGN] the server sent no connection's ID"))
            }
        }
    }

    /// \[EXTENSION\] This connection attached as the debugger of the session
    /// of connection `id`: one of its own user's, or any if its user is
    /// the database's owner or holds MONITOR (RULES.md D1031).
    ///
    /// # Errors
    ///
    /// SQLSTATE 08003 if no connection of that ID is served whose session
    /// this one may debug; TBD01 if this connection is attached already,
    /// or the session is this one's, or has a debugger already.
    pub fn debug_attach(&mut self, id: u64) -> Result<()> {
        self.debug_done(DebugRequest::Attach(id))
    }

    /// \[EXTENSION\] This connection detached: its breakpoints gone, its
    /// target, if paused, going on.
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if it is attached to none.
    pub fn debug_detach(&mut self) -> Result<()> {
        self.events_dropped();
        self.debug_done(DebugRequest::Detach)
    }

    /// \[EXTENSION\] A breakpoint at line `line`, from 1, of the text of
    /// the routine of `kind` named `routine`, its specific name or, if it
    /// is the one routine of that kind so named, its name; or of the
    /// trigger `routine`. An empty schema is the session's default. The
    /// target pauses at each statement that begins on that line, where
    /// `condition`, a `<search condition>` over the frame's parameters and
    /// variables, is true if given.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such routine or line, -104 if the user
    /// holds no DEBUG on it; TBD01 if this connection is attached to none.
    pub fn debug_break(&mut self, (kind, routine): (Kind, &QualifiedName), line: u32, condition: Option<&str>) -> Result<DebugBreakpoint> {
        let request = DebugRequest::Break { kind, routine: routine.clone(), line, condition: condition.map(str::to_owned) };
        match self.debugged(request)? {
            DebugReply::Breakpoint(set) => Ok(set),
            DebugReply::Connection(_) | DebugReply::Breakpoints(_) | DebugReply::Stack(_) => {
                Err(self.lose("[DESIGN] the server sent no breakpoint"))
            }
        }
    }

    /// \[EXTENSION\] The breakpoint `number` cleared.
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if there is none of that number, or this connection
    /// is attached to none.
    pub fn debug_clear(&mut self, number: u32) -> Result<()> {
        self.debug_done(DebugRequest::Clear(number))
    }

    /// \[EXTENSION\] The breakpoints set, in the order they were set.
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if this connection is attached to none.
    pub fn debug_breakpoints(&mut self) -> Result<Vec<DebugBreakpoint>> {
        match self.debugged(DebugRequest::Breakpoints)? {
            DebugReply::Breakpoints(all) => Ok(all),
            DebugReply::Connection(_) | DebugReply::Breakpoint(_) | DebugReply::Stack(_) => {
                Err(self.lose("[DESIGN] the server sent no breakpoints"))
            }
        }
    }

    /// \[EXTENSION\] The paused target told to go on as `how` says: to the
    /// next statement, into a routine it invokes (`Into`) or not (`Over`),
    /// to the next of the frame it returns to (`Out`), or to a breakpoint
    /// (`Continue`).
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if the target is not paused, or this connection is
    /// attached to none.
    pub fn debug_resume(&mut self, how: Action) -> Result<()> {
        self.debug_done(DebugRequest::Resume(how))
    }

    /// \[EXTENSION\] The running target told to pause at the next statement
    /// it runs of a routine the debugger may debug.
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if this connection is attached to none.
    pub fn debug_interrupt(&mut self) -> Result<()> {
        self.debug_done(DebugRequest::Interrupt)
    }

    /// \[EXTENSION\] The target's pause, with its call stack, the
    /// outermost frame first; none if it runs.
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if this connection is attached to none.
    pub fn debug_stack(&mut self) -> Result<Option<Paused>> {
        match self.debugged(DebugRequest::Stack)? {
            DebugReply::Stack(paused) => Ok(paused),
            DebugReply::Connection(_) | DebugReply::Breakpoint(_) | DebugReply::Breakpoints(_) => {
                Err(self.lose("[DESIGN] the server sent no stack"))
            }
        }
    }

    /// \[EXTENSION\] The next event of the session this connection debugs
    /// not yet read: one kept, else the next the server sends, waited for
    /// at most `within`, without limit if none; none if none came. A
    /// connection made over a stream given to `open` waits only without
    /// limit. It is read between requests, never while one is answered.
    ///
    /// # Errors
    ///
    /// A connection failure: `within` given of a stream given to `open`,
    /// the server gone, or a reply where an event was waited for.
    pub fn debug_wait(&mut self, within: Option<Duration>) -> Result<Option<Event>> {
        if let Some(kept) = self.told.events.pop_front() {
            return Ok(Some(kept));
        }
        if !self.arrives_within(within)? {
            return Ok(None);
        }
        match self.next_reply(Answer::One)? {
            Reply::Event(event) => Ok(Some(event)),
            Reply::Notice(notice) => {
                self.told.notice(notice);
                Ok(None)
            }
            Reply::Status { name, value } => {
                self.told.settings.insert(name, value);
                Ok(None)
            }
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Complete(_)
            | Reply::Pong
            | Reply::Debug(_) => Err(self.lose("[DESIGN] the server answered no request")),
        }
    }

    /// Whether a frame begins to arrive within `within`, without limit if
    /// none: nothing of it read, the connection's own timeout put back.
    fn arrives_within(&mut self, within: Option<Duration>) -> Result<bool> {
        let kept = self.read_timeout();
        if within != kept {
            self.set_read_timeout(within)?;
        }
        let filled = uninterrupted(|| self.stream.fill_buf().map(|bytes| !bytes.is_empty()));
        if within != kept {
            self.set_read_timeout(kept)?;
        }
        match filled {
            Ok(true) => Ok(true),
            Ok(false) => Err(self.lose("[DESIGN] the server closed the connection")),
            Err(e) => {
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
                    return Ok(false);
                }
                Err(self.lost_by(io_failed("receive", &e)))
            }
        }
    }

    /// The events kept, dropped: of a target the connection no longer
    /// debugs.
    fn events_dropped(&mut self) {
        self.told.events.clear();
    }
}
