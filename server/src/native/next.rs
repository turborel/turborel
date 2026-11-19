//! A client's next request, once it is authenticated: work for a worker,
//! or what the connection's task does itself. Ping is answered here,
//! with no worker (RULES.md D189); Reset and ChangeUser renew the session
//! (`native/renew.rs`). A request of a capability the client was not
//! granted is a connection failure (D185): a client never sends one.

use std::sync::Arc;
use std::time::Instant;

use turborel::error::Result;
use turborel_protocol::capability::{CHANGE_USER, COPY, CURSORS, DESCRIBE, MULTI_STATEMENT, PING, QUERY_ATTRIBUTES, RESET};
use turborel_protocol::{Attributes, Capabilities, Reply, Request, broken};

use super::lent::{self, Lent};
use super::requests::Work;
use crate::server::Services;
use crate::shown::Present;

/// What the connection's task is asked next.
pub(super) enum Next {
    /// Work for a worker.
    Work(Work),
    /// The session's end.
    Terminate,
    /// The session back to its state at login (RULES.md D189).
    Reset,
    /// The connection authenticated again: as `user`, by `mechanism`, its
    /// first bytes `initial`.
    ChangeUser { user: String, mechanism: String, initial: Vec<u8> },
}

/// `attributes`, as a request carries them: none if empty.
fn carried(attributes: Attributes) -> Option<Arc<Attributes>> {
    (!attributes.is_empty()).then(|| Arc::new(attributes))
}

/// The capabilities `request` needs: up to two, each if it uses it.
fn needs(request: &Request) -> [Option<&'static str>; 2] {
    let one = |name| [Some(name), None];
    match request {
        Request::Query { attributes, multi, .. } => {
            [(!attributes.is_empty()).then_some(QUERY_ATTRIBUTES), multi.then_some(MULTI_STATEMENT)]
        }
        Request::Execute { attributes, cursor, .. } => {
            [(!attributes.is_empty()).then_some(QUERY_ATTRIBUTES), (*cursor != 0).then_some(CURSORS)]
        }
        Request::Fetch { cursor, .. } | Request::Close { cursor, .. } => [(*cursor != 0).then_some(CURSORS), None],
        Request::Describe(_) => one(DESCRIBE),
        Request::Ping => one(PING),
        Request::Reset => one(RESET),
        Request::ChangeUser { .. } => one(CHANGE_USER),
        Request::CopyIn { .. } | Request::CopyData(_) | Request::CopyEnd { .. } => one(COPY),
        Request::Hello { .. }
        | Request::Answer(_)
        | Request::Prepare { .. }
        | Request::Deallocate { .. }
        | Request::Lob(_)
        | Request::Cancel(_)
        | Request::Terminate => [None, None],
    }
}

/// What `request` asks, of a client granted `granted`; none for Ping,
/// which the caller answers.
///
/// # Errors
///
/// A connection failure: a request of a capability not granted, or one
/// that comes only before the client is authenticated.
pub(super) fn next_of(request: Request, granted: Capabilities) -> Result<Option<Next>> {
    if let Some(name) = needs(&request).into_iter().flatten().find(|name| !granted.has(name)) {
        return Err(broken(format!("[DESIGN] a request of {name}, which the client was not granted")));
    }
    Ok(Some(Next::Work(match request {
        Request::Query { sql, ends, attributes, multi } => Work::Query { sql, ends, attributes: carried(attributes), multi },
        Request::Prepare { statement, sql } => Work::Prepare(statement, sql),
        Request::Execute { statement, cursor, values, rows, ends, attributes } => {
            Work::Execute { at: (statement, cursor), values, rows, ends, attributes: carried(attributes) }
        }
        Request::Fetch { statement, cursor, rows } => Work::Fetch((statement, cursor), rows),
        Request::Close { statement, cursor } => Work::Close((statement, cursor)),
        Request::Deallocate { statement } => Work::Deallocate(statement),
        Request::Lob(request) => Work::Lob(request),
        Request::Describe(what) => Work::Describe(what),
        Request::CopyIn { table, columns, ends } => Work::CopyIn { table, columns, ends },
        Request::CopyData(rows) => Work::CopyData(rows),
        Request::CopyEnd { abandon } => Work::CopyEnd(abandon),
        Request::Ping => return Ok(None),
        Request::Terminate => return Ok(Some(Next::Terminate)),
        Request::Reset => return Ok(Some(Next::Reset)),
        Request::ChangeUser { user, mechanism, initial } => return Ok(Some(Next::ChangeUser { user, mechanism, initial })),
        Request::Hello { .. } | Request::Answer(_) => return Err(broken("[DESIGN] a connection is authenticated once")),
        Request::Cancel(_) => return Err(broken("[DESIGN] a Cancel is a connection's first and only message")),
    })))
}

/// The work `request` asks of a worker, of a client granted `granted`:
/// none for anything else, or a request in error, left for the
/// connection's task (`Lent::next`).
pub(super) fn work_of(request: Request, granted: Capabilities) -> Option<Work> {
    match next_of(request, granted) {
        Ok(Some(Next::Work(work))) => Some(work),
        Ok(Some(Next::Terminate | Next::Reset | Next::ChangeUser { .. }) | None) | Err(_) => None,
    }
}

/// What the client asks next, each Ping answered meanwhile: waited for
/// SERVER.IDLE_TIMEOUT_MS, or SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS too
/// while the session's transaction is active; the client fast if it came
/// within `lent::WAITED` (`Lent::fast`). Refused (SQLSTATE 28000) if the
/// session is `user`'s, and the user has been dropped since it logged in.
pub(super) async fn next_work(lent: &mut Lent, (services, present): (&Services, &Present), user: Option<&str>) -> Result<Next> {
    loop {
        let began = Instant::now();
        let receiving = super::next(&mut lent.wire, services, present);
        let request = if lent.held.session.transaction().is_some() {
            services.idle_in_transaction(receiving).await
        } else {
            services.idle(receiving).await
        };
        lent.fast = began.elapsed() < lent::WAITED;
        let Some(next) = next_of(request?, lent.held.granted)? else {
            services.idle(lent.wire.send_now(&Reply::Pong)).await?;
            continue;
        };
        lent.held.arrived();
        let Some(name) = user else { return Ok(next) };
        if services.database().is_user(name)? {
            return Ok(next);
        }
        return Err(super::refused(format!("[DESIGN] the user {name} was dropped: its connections end")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D185: a request of a capability not
    /// granted fails the connection; granted, it is taken; Ping is the
    /// task's to answer, and Reset the task's to do, never a worker's.
    #[test]
    fn a_request_needs_what_it_uses_granted() {
        let query = |multi| Request::Query { sql: "VALUES 1".into(), ends: false, attributes: Vec::new(), multi };
        assert!(next_of(query(true), Capabilities::NONE).is_err(), "multi not granted");
        assert!(matches!(next_of(query(true), Capabilities::of(&[MULTI_STATEMENT])), Ok(Some(Next::Work(_)))));
        assert!(matches!(next_of(query(false), Capabilities::NONE), Ok(Some(Next::Work(_)))));
        let fetch = Request::Fetch { statement: 1, cursor: 2, rows: 0 };
        assert!(next_of(fetch, Capabilities::NONE).is_err(), "a numbered cursor not granted");
        assert!(matches!(next_of(Request::Ping, Capabilities::of(&[PING])), Ok(None)));
        assert!(next_of(Request::Ping, Capabilities::NONE).is_err());
        assert!(work_of(Request::Reset, Capabilities::all()).is_none());
        assert!(work_of(Request::Terminate, Capabilities::all()).is_none());
    }
}
