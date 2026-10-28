//! \[DESIGN\] The native protocol's server side (RULES.md D42, D45; the
//! messages in `turborel_protocol`): a connection authenticates, then runs
//! its requests in its session, one at a time, in order; or, its first and
//! only message a Cancel, cancels the statement another connection runs.
//!
//! A request runs on a worker, which owns the session, its prepared
//! statements and their open cursors while it runs (`native/requests.rs`).
//! Its replies go from the worker to the connection's task through a
//! bounded channel, so a client reading slowly holds the worker but never
//! more than a few batches of memory; a client gone closes the channel,
//! and the worker stops fetching. The session ends on a worker too: as
//! SESSION_END says once the client terminates it; rolled back if the
//! connection ends otherwise, lost or failed or cut by the server stopping,
//! as a direct invocation of SQL that terminated unexpectedly [ISO/IEC
//! 9075:1992 §20.1 GR2a].
//!
//! A connection has SERVER.LOGIN_TIMEOUT to authenticate, and is closed,
//! its session rolled back, once it has kept the server waiting
//! SERVER.IDLE_TIMEOUT for a request, or to take what it is sent.

mod requests;
mod wire;

use requests::{Held, Work};
use tokio::sync::mpsc;
use turborel::db::Session;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::value::types::TypeRegistry;
use turborel_protocol::{Reply, Request, VERSION, broken};
use wire::Wire;

use crate::auth::{Peer, Step};
use crate::server::{Connection, Protocol, Registered, Services, Serving};
use crate::settings;
use crate::transport::Transport;

/// \[DESIGN\] Batches of rows in flight between a worker and its
/// connection.
const IN_FLIGHT: usize = 4;

/// \[DESIGN\] The native protocol.
#[derive(Debug, Clone, Copy, Default)]
pub struct Native;

impl Protocol for Native {
    fn name(&self) -> &'static str {
        "native"
    }

    fn serve(&self, connection: Connection) -> Serving {
        Box::pin(serve(connection))
    }

    fn refuse(&self, transport: Box<dyn Transport>, why: SqlError) -> Serving {
        Box::pin(async move {
            let mut wire = Wire::new(transport, 0, TypeRegistry::default());
            wire.close_with(&why).await;
        })
    }
}

/// The client's next request, unless the server stops first.
async fn next(wire: &mut Wire, services: &Services) -> Result<Request> {
    tokio::select! {
        request = wire.receive() => request,
        () = services.stopping() => Err(broken("[DESIGN] the server is stopping")),
    }
}

/// Serves `connection` to its end.
async fn serve(connection: Connection) {
    let Connection { transport, peer, services } = connection;
    let max_body = u32::try_from(settings::count(services.database(), settings::MAX_MESSAGE_BYTES)).unwrap_or(u32::MAX);
    // A client's values of types beyond the standard read as the
    // database's (RULES.md D53).
    let types = services.database().types().unwrap_or_default();
    let mut wire = Wire::new(transport, max_body, types);
    let ended = match services.login(welcome(&mut wire, &peer, &services)).await {
        // The key cancels the session's statements while it lives.
        Ok(Some((session, _key))) => run(&mut wire, &services, session).await,
        Ok(None) => Ok(()),
        Err(e) => Err(e),
    };
    // Told within SERVER.IDLE_TIMEOUT, as any reply: a client that takes
    // nothing would hold the connection open.
    let closing = async {
        match ended {
            Ok(()) => wire.close().await,
            Err(e) => wire.close_with(&e).await,
        }
        Ok(())
    };
    let _ = services.idle(closing).await;
}

/// A refusal of the connection's establishment (SQLSTATE 08004).
fn rejected(msg: String) -> SqlError {
    SqlError::new(Code::Connection, msg).with_state(SqlState::CONNECTION_REJECTED)
}

/// A refusal of credentials (SQLSTATE 28000).
fn refused(msg: String) -> SqlError {
    SqlError::new(Code::Connection, msg).with_state(SqlState::INVALID_AUTHORIZATION)
}

/// The client's first request: Hello, its session once authenticated, with
/// its cancel key registered; or a Cancel, done here, and nothing more.
async fn welcome(wire: &mut Wire, peer: &Peer, services: &Services) -> Result<Option<(Session, Registered)>> {
    match next(wire, services).await? {
        Request::Cancel(key) => {
            services.cancel(&key);
            Ok(None)
        }
        // No capability is defined in version 1, so none is taken.
        Request::Hello { version, user, mechanism, initial, capabilities: _ } => {
            authenticate(wire, peer, services, (version, user, mechanism, initial)).await.map(Some)
        }
        Request::Answer(_)
        | Request::Query(_)
        | Request::Prepare { .. }
        | Request::Execute { .. }
        | Request::Fetch { .. }
        | Request::Close { .. }
        | Request::Deallocate { .. }
        | Request::Terminate => Err(broken("[DESIGN] a connection begins with Hello, or Cancel")),
    }
}

/// Authenticates the client of `hello`: its version, user, mechanism and
/// first bytes; its session, once Ready is sent with its cancel key.
async fn authenticate(
    wire: &mut Wire,
    peer: &Peer,
    services: &Services,
    hello: (u32, String, String, Vec<u8>),
) -> Result<(Session, Registered)> {
    let (version, user, mechanism, initial) = hello;
    if version != VERSION {
        return Err(rejected(format!("[DESIGN] protocol version {version} is not served; this server speaks {VERSION}")));
    }
    // [ISO/IEC 9075:1992 §5.4] the user is an <authorization identifier>:
    // an <identifier>, a regular one in upper case.
    let user = turborel::lexer::authorization_identifier(&user).map_err(|e| refused(format!("[DESIGN] {}", e.msg)))?;
    let authenticator =
        services.authenticator(&mechanism).ok_or_else(|| refused(format!("[DESIGN] mechanism {mechanism} is not served")))?;
    let mut exchange = authenticator.begin(&user, peer);
    let mut input = initial;
    let authorization = loop {
        let (taken, step) = services
            .run(move || {
                let step = exchange.step(&input);
                (exchange, step)
            })
            .await?;
        exchange = taken;
        match step {
            Step::Challenge(challenge) => wire.send_now(&Reply::Challenge(challenge)).await?,
            Step::Accept(authorization) => break authorization,
            Step::Refuse(why) => return Err(refused(why)),
        }
        let Request::Answer(answer) = next(wire, services).await? else {
            return Err(broken("[DESIGN] a challenge is answered with Answer"));
        };
        input = answer;
    };
    let session = services.database().session(&authorization);
    let key = services.register_cancel(session.canceller())?;
    wire.send_now(&Reply::Ready { authorization, key: key.key, capabilities: Vec::new(), server: crate::SERVER.to_owned() }).await?;
    Ok((session, key))
}

/// The work `request` asks of the session; none for Terminate.
fn work_of(request: Request) -> Result<Option<Work>> {
    Ok(Some(match request {
        Request::Query(sql) => Work::Query(sql),
        Request::Prepare { statement, sql } => Work::Prepare(statement, sql),
        Request::Execute { statement, values, rows } => Work::Execute(statement, values, rows),
        Request::Fetch { statement, rows } => Work::Fetch(statement, rows),
        Request::Close { statement } => Work::Close(statement),
        Request::Deallocate { statement } => Work::Deallocate(statement),
        Request::Terminate => return Ok(None),
        Request::Hello { .. } | Request::Answer(_) => return Err(broken("[DESIGN] a connection is authenticated once")),
        Request::Cancel(_) => return Err(broken("[DESIGN] a Cancel is a connection's first and only message")),
    }))
}

/// Runs the client's requests in `session` until it terminates, the
/// connection fails or the server stops; then ends the session on a
/// worker, rolled back unless the client terminated it [ISO/IEC 9075:1992
/// §20.1 GR2a].
async fn run(wire: &mut Wire, services: &Services, opened: Session) -> Result<()> {
    let mut held = Some(Held::new(opened));
    let ended = loop {
        let work = match services.idle(next(wire, services)).await.and_then(work_of) {
            Ok(Some(work)) => work,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        let Some(lent) = held.take() else { break Err(broken("[DESIGN] the session is gone")) };
        match exchange(wire, services, lent, work).await {
            Ok(back) => held = Some(back),
            Err(e) => break Err(e),
        }
    };
    if let Some(back) = held {
        end(services, back, ended.is_err()).await?;
    }
    ended
}

/// Ends `held`'s session on a worker: rolled back first if it ended
/// `unexpected`ly [ISO/IEC 9075:1992 §20.1 GR2a].
async fn end(services: &Services, held: Held, unexpected: bool) -> Result<()> {
    services
        .run(move || {
            if unexpected {
                // A failed rollback has marked its tables damaged (RULES.md
                // D19): nothing is left for the client.
                let _ = held.session.rollback();
            }
            drop(held);
        })
        .await
}

/// Does `work` with `lent` on a worker, sending its replies as they come:
/// what it holds, given back.
async fn exchange(wire: &mut Wire, services: &Services, lent: Held, work: Work) -> Result<Held> {
    let (sender, mut receiver) = mpsc::channel(IN_FLIGHT);
    let db = services.database();
    let (batch, max_statements) = (settings::count(db, settings::BATCH_BYTES), settings::count(db, settings::MAX_STATEMENTS));
    let mut held = lent;
    let running = services.run(move || {
        held.handle(work, (&sender, batch), max_statements);
        held
    });
    let forward = async move {
        while let Some(reply) = receiver.recv().await {
            // A client that stops taking what it is sent holds a worker:
            // it is given SERVER.IDLE_TIMEOUT, as one sending nothing is.
            services.idle(wire.send(&reply)).await?;
            if receiver.is_empty() {
                services.idle(wire.flush()).await?;
            }
        }
        Ok::<(), SqlError>(())
    };
    let (back, forwarded) = tokio::join!(running, forward);
    let back = back?;
    if let Err(e) = forwarded {
        // As `run`'s: the connection is lost.
        end(services, back, true).await?;
        return Err(e);
    }
    Ok(back)
}
