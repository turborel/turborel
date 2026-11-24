//! \[EXTENSION\] A debugger's requests, run on a worker in its own
//! session (RULES.md D1030, D1031): Identify tells the connection its ID;
//! Attach finds the session of another connection by its ID and attaches
//! to it; the rest are the database's debugger's calls, refused
//! (SQLSTATE TBD01) while the connection is attached to none.

use turborel::db::{Completion, DebugTarget, Debugger};
use turborel::debugger::Breakpoint;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel_protocol::{DebugBreakpoint, DebugReply, DebugRequest, Reply};

use super::Held;
use crate::native::debugging::{Debugging, refused};

/// \[EXTENSION\] RULES.md D1031: no connection of ID `id` this session may
/// debug is served (SQLSTATE 08003), told alike whether none is, or one is
/// that its user may not see, so a user learns nothing of another's.
fn no_connection(id: u64) -> SqlError {
    let msg = format!("[EXTENSION] no connection {id} is served whose session this one may debug");
    SqlError::new(Code::Connection, msg).with_state(SqlState::CONNECTION_DOES_NOT_EXIST)
}

/// A breakpoint, as the client is told it.
fn told(b: Breakpoint) -> DebugBreakpoint {
    DebugBreakpoint { number: b.number, routine: b.routine, line: b.line, condition: b.condition }
}

/// A request done with nothing to give back.
fn done() -> Reply {
    Reply::Complete(Completion::Success.into())
}

impl Held {
    /// \[EXTENSION\] Does the debugger's `request`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08003 if Attach names no connection this session may
    /// debug; TBD01 if the connection is attached already, or not, or the
    /// debugger refuses it; as the debugger's call fails.
    pub(super) fn debug(&mut self, request: DebugRequest) -> Result<Reply> {
        let debugger = match request {
            DebugRequest::Identify => {
                let id = self.serves.present.as_ref().map_or(0, crate::shown::Present::id);
                return Ok(Reply::Debug(DebugReply::Connection(id)));
            }
            DebugRequest::Attach(id) => return self.attach(id).map(|()| done()),
            DebugRequest::Detach => {
                return self.debugging.take().map(|_| done()).ok_or_else(not_attached);
            }
            DebugRequest::Break { .. }
            | DebugRequest::Clear(_)
            | DebugRequest::Breakpoints
            | DebugRequest::Resume(_)
            | DebugRequest::Interrupt
            | DebugRequest::Stack => self.debugging.as_ref().map(|d| std::sync::Arc::clone(&d.debugger)).ok_or_else(not_attached)?,
        };
        self.called(&debugger, request)
    }

    /// The call `request` makes of `debugger`, attached.
    fn called(&self, debugger: &Debugger, request: DebugRequest) -> Result<Reply> {
        match request {
            DebugRequest::Break { kind, routine, line, condition } => {
                let set = debugger.break_at(&self.session, (kind, &routine), line, condition.as_deref())?;
                Ok(Reply::Debug(DebugReply::Breakpoint(told(set))))
            }
            DebugRequest::Clear(number) => debugger.clear(number).map(|()| done()),
            DebugRequest::Breakpoints => Ok(Reply::Debug(DebugReply::Breakpoints(debugger.breakpoints()?.into_iter().map(told).collect()))),
            DebugRequest::Resume(how) => debugger.resume(how).map(|()| done()),
            DebugRequest::Interrupt => debugger.interrupt().map(|()| done()),
            DebugRequest::Stack => Ok(Reply::Debug(DebugReply::Stack(debugger.paused()?))),
            DebugRequest::Identify | DebugRequest::Attach(_) | DebugRequest::Detach => Ok(done()),
        }
    }

    /// \[EXTENSION\] RULES.md D1031: this session attached as the debugger
    /// of the session of connection `id`: one of its own user's, or any if
    /// its user sees every session (`Session::may_monitor`).
    fn attach(&mut self, id: u64) -> Result<()> {
        if self.debugging.is_some() {
            return Err(refused("this connection debugs a session already: detach it first"));
        }
        let present = self.serves.present.as_ref().ok_or_else(|| no_connection(id))?;
        let (user, stop) = present.session_of(id).ok_or_else(|| no_connection(id))?;
        let mine = user.as_deref() == Some(self.session.current_authorization().as_str());
        if !mine && !self.session.may_monitor() {
            return Err(no_connection(id));
        }
        let debugger = self.session.attach(&DebugTarget::of(stop))?;
        let judge = self.serves.db.session(&self.session.current_authorization());
        self.debugging = Some(Box::new(Debugging::new(debugger, judge)));
        Ok(())
    }
}

/// A debugger's call on a connection attached to no session (SQLSTATE
/// TBD01).
fn not_attached() -> SqlError {
    refused("this connection is attached to no session: attach it first")
}
