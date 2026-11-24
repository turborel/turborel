//! \[EXTENSION\] A connection that debugs another's session (RULES.md
//! D1030 to D1034): the debugger it holds while attached, and what it
//! hears of its target, sent to its client unasked while the connection
//! waits for a request, never within a request's replies (D1032).
//!
//! The database's debugger is waited on, blocking, a slice at a time
//! (`SLICE`), on a thread of the runtime's for blocking work, apart from
//! the workers. The wait under way is kept, not dropped, when a request
//! comes first, and looked at again once the connection next waits, so no
//! event is lost between them (D1033). The requests themselves run on a
//! worker, as any request does (`requests/debugged.rs`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use turborel::db::{Debugger, Session};
use turborel::debugger::Event;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel_protocol::{Reply, Request};

use super::lent::Lent;
use crate::server::Services;
use crate::shown::Present;

/// \[DESIGN\] RULES.md D1033: the longest one wait for the debugger's
/// next event blocks its thread, as often as a connection whose request
/// runs is looked at for its client gone (`LOOK_AGAIN`). A debugger
/// detached goes once the wait under way ends, so its target hears of
/// the detach within it.
const SLICE: Duration = Duration::from_millis(100);

/// What a wait for the debugger's next event gives back: the session
/// that judged its breakpoints' conditions, and the event, if one came.
type Waited = (Session, Result<Option<Event>>);

/// \[EXTENSION\] What a connection attached as a debugger holds.
pub(crate) struct Debugging {
    /// The database's debugger, shared with the wait under way.
    pub(super) debugger: Arc<Debugger>,
    /// \[DESIGN\] RULES.md D1033: a session of the debugger's user, apart
    /// from the connection's, in which the wait judges a breakpoint's
    /// condition; none while a wait has it.
    judge: Option<Session>,
    /// The wait for its next event under way, if one is.
    waiting: Option<JoinHandle<Waited>>,
}

/// A debugger's request its state does not allow (SQLSTATE TBD01), as
/// the database's debugger refuses one.
pub(super) fn refused(msg: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] {msg}")).with_state(SqlState::DEBUGGER)
}

impl Debugging {
    /// Of `debugger`, just attached, its conditions judged in `judge`, no
    /// wait under way yet.
    pub(super) fn new(debugger: Debugger, judge: Session) -> Debugging {
        Debugging { debugger: Arc::new(debugger), judge: Some(judge), waiting: None }
    }

    /// The debugger's next event: the wait under way, if one is, else a
    /// new one, slice after slice until one comes. Dropped before it is
    /// done, the wait under way stays, so its event is the next this
    /// gives.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the debugger's lock is poisoned or the wait could
    /// not run.
    async fn next_event(&mut self) -> Result<Event> {
        loop {
            if self.waiting.is_none() {
                let judge = self.judge.take().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the debugger's wait lost its session"))?;
                let debugger = Arc::clone(&self.debugger);
                self.waiting = Some(tokio::task::spawn_blocking(move || {
                    let event = debugger.wait(&judge, SLICE);
                    (judge, event)
                }));
            }
            let Some(waiting) = self.waiting.as_mut() else { continue };
            let waited = waiting.await;
            self.waiting = None;
            let (judge, event) = waited.map_err(|e| SqlError::new(Code::Api, format!("[DESIGN] the debugger's wait failed: {e}")))?;
            self.judge = Some(judge);
            if let Some(event) = event? {
                return Ok(event);
            }
        }
    }
}

/// What a connection waiting for its client's next request hears first.
pub(super) enum Heard {
    /// The client's request, or the failure of reading it.
    Request(Result<Request>),
    /// An event of the session it debugs, sent to it.
    Event,
}

/// \[EXTENSION\] RULES.md D1032: the client's next request on `lent`'s
/// wire, waited for as `Services::idle_since` says from `since`, in a
/// transaction or not; or, of a connection attached as a debugger, an
/// event of its target's that came first, sent to the client, the read
/// then dropped, as nothing of it was taken. The debugger goes once its
/// target ends, or its wait fails, the client told Ended.
///
/// # Errors
///
/// A write of the event failed: the connection is gone.
pub(super) async fn heard(
    lent: &mut Lent,
    (services, present): (&Services, &Present),
    (since, in_transaction): (Instant, bool),
) -> Result<Heard> {
    let Lent { held, wire, .. } = lent;
    let receiving = services.idle_since(since, in_transaction, super::next(wire, services, present));
    let Some(debugging) = held.debugging.as_mut() else { return Ok(Heard::Request(receiving.await)) };
    let event = tokio::select! {
        request = receiving => return Ok(Heard::Request(request)),
        event = debugging.next_event() => event,
    };
    let event = match event {
        Ok(Event::Ended) | Err(_) => {
            held.debugging = None;
            Event::Ended
        }
        Ok(event @ (Event::Paused(_) | Event::TimedOut | Event::Cancelled)) => event,
    };
    services.idle(wire.send_now(&Reply::Event(event))).await?;
    Ok(Heard::Event)
}
