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
//! SESSION.COMMIT_AT_END says once the client terminates it, a commit that fails
//! sent as the client's error before the connection closes; rolled back
//! if the connection ends otherwise, lost or failed, cut by the server
//! stopping or ended by its operator, as a direct invocation of SQL that
//! terminated unexpectedly [ISO/IEC 9075:1992 §20.1 GR2a].
//!
//! A connection has SERVER.LOGIN_TIMEOUT_MS to authenticate, in messages of
//! at most `LOGIN_MESSAGE_BYTES`, its checks run apart from the workers
//! and slowed after failures from its address (`logins.rs`). It is closed,
//! its session rolled back, once it has kept the server waiting
//! SERVER.IDLE_TIMEOUT_MS for a request, or to take what it is sent, or
//! SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS for a request while its
//! transaction is active. A session of a user is ended at its next request
//! once the user is dropped (RULES.md D43).

mod requests;
mod wire;

use std::future::Future;
use std::sync::Arc;

use requests::{Held, Work};
use tokio::sync::mpsc;
use turborel::db::Session;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::value::types::TypeRegistry;
use turborel_protocol::{Reply, Request, VERSION, broken};
use wire::Wire;

use crate::auth::{Authenticator, Exchange, Peer, Step};
use crate::server::{Connection, Protocol, Registered, Services, Serving};
use crate::settings;
use crate::shown::Present;
use crate::told;
use crate::transport::Transport;

/// \[DESIGN\] The batches of replies in flight on a connection, from the
/// worker running its statement to the connection sending them: those a
/// worker makes before it waits for the connection to take one. Fixed,
/// not a setting (RULES.md D36): the shape of the channel made for each
/// statement; what they hold is SERVER.BATCH_BYTES each, the setting.
const IN_FLIGHT: usize = 4;

/// \[DESIGN\] How often a statement of a connection its operator ended is
/// cancelled again until it stops: a cancel made before it began does
/// nothing (RULES.md D44). As often as a wait for a lock looks.
const CANCEL_AGAIN: std::time::Duration = std::time::Duration::from_millis(20);

/// \[DESIGN\] The longest message taken from a client before it is
/// authenticated, or SERVER.MAX_MESSAGE_BYTES if that is less: room for
/// Hello, an Answer or a Cancel, each a user's name, a mechanism's and
/// its proof; so a connection not yet authenticated holds little memory.
pub const LOGIN_MESSAGE_BYTES: u32 = 16 * 1024;

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

/// The ending of a connection by the server's operator (SQLSTATE 08006).
fn ended() -> SqlError {
    broken("[DESIGN] the server's operator ended the connection")
}

/// The client's next request, unless the server stops, or its operator
/// ends the connection, first.
async fn next(wire: &mut Wire, services: &Services, present: &Present) -> Result<Request> {
    tokio::select! {
        biased;
        () = present.ended() => Err(ended()),
        () = services.stopping() => Err(broken("[DESIGN] the server is stopping")),
        request = wire.receive() => request,
    }
}

/// What `worked` gives back, once it is done: if the operator ends the
/// connection first, what its session runs cancelled, again every
/// `CANCEL_AGAIN` until it stops.
async fn stopped<T>(worked: impl Future<Output = T>, present: &Present) -> T {
    tokio::pin!(worked);
    tokio::select! {
        back = &mut worked => return back,
        () = present.ended() => {}
    }
    loop {
        present.cancel();
        if let Ok(back) = tokio::time::timeout(CANCEL_AGAIN, &mut worked).await {
            return back;
        }
    }
}

/// A client authenticated: its session, its cancel key, registered while
/// this lives, and the user it logged in as, if that was a user.
struct Admitted {
    session: Session,
    key: Registered,
    user: Option<String>,
}

/// Serves `connection` to its end.
async fn serve(connection: Connection) {
    let Connection { transport, peer, services, present, arrived } = connection;
    let max_body = u32::try_from(settings::count(services.database(), settings::MAX_MESSAGE_BYTES)).unwrap_or(u32::MAX);
    // A client's values of types beyond the standard read as the
    // database's (RULES.md D53).
    let types = services.database().types().unwrap_or_default();
    let mut wire = Wire::new(transport, max_body.min(LOGIN_MESSAGE_BYTES), types);
    let ended = match services.login(arrived, welcome(&mut wire, (&peer, &present), &services)).await {
        Ok(Some(admitted)) => {
            wire.set_max_body(max_body);
            run(&mut wire, (&services, &present), admitted).await
        }
        Ok(None) => Ok(()),
        Err(e) => Err(e),
    };
    // Told within SERVER.IDLE_TIMEOUT_MS, as any reply: a client that takes
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
async fn welcome(wire: &mut Wire, (peer, present): (&Peer, &Present), services: &Services) -> Result<Option<Admitted>> {
    match next(wire, services, present).await? {
        Request::Cancel(key) => {
            services.cancel(&key);
            Ok(None)
        }
        // No capability is defined in version 1, so none is taken.
        Request::Hello { version, user, mechanism, initial, capabilities: _ } => {
            authenticate(wire, (peer, present), services, (version, user, mechanism, initial)).await.map(Some)
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
/// first bytes; its session, once Ready is sent with its cancel key, and
/// its user shown (`Present`), its statements cancelled as an operator
/// ends it. Its address waits its turn after failed logins first
/// (`logins.rs`), so a Hello refused before any authenticator runs is
/// slowed, counted and told as a proof refused is.
async fn authenticate(
    wire: &mut Wire,
    (peer, present): (&Peer, &Present),
    services: &Services,
    hello: (u32, String, String, Vec<u8>),
) -> Result<Admitted> {
    let (version, given, mechanism, initial) = hello;
    let (logins, db) = (services.logins(), services.database());
    logins.wait_turn(db, peer).await;
    let (user, authenticator) = match admitted(services, version, &given, &mechanism) {
        Ok(admitted) => admitted,
        Err(e) => {
            logins.failed(db, peer);
            told::hello_refused(db, (&given, peer, &mechanism), &e);
            return Err(e);
        }
    };
    let exchange = authenticator.begin(&user, peer);
    let authorization = exchanged(wire, (peer, present), services, (exchange, initial)).await?;
    // A user's session lasts while the user does (RULES.md D43).
    let member = db.is_user(&authorization)?.then(|| authorization.clone());
    let session = db.session(&authorization);
    let key = services.register_cancel(session.canceller())?;
    present.stoppable(session.canceller());
    present.authenticated(&authorization);
    wire.send_now(&Reply::Ready { authorization, key: key.key, capabilities: Vec::new(), server: turborel::product() }).await?;
    Ok(Admitted { session, key, user: member })
}

/// The user a Hello of `version`, naming `user` and `mechanism`, logs in
/// as, and the authenticator of its mechanism; refused, before any
/// authenticator runs, if the version is not served (SQLSTATE 08004), or
/// the user is no identifier, or the mechanism is not served (28000).
fn admitted(services: &Services, version: u32, user: &str, mechanism: &str) -> Result<(String, Arc<dyn Authenticator>)> {
    if version != VERSION {
        return Err(rejected(format!("[DESIGN] protocol version {version} is not served; this server speaks {VERSION}")));
    }
    // [ISO/IEC 9075:1992 §5.4] the user is an <authorization identifier>:
    // an <identifier>, a regular one in upper case.
    let user = turborel::lexer::authorization_identifier(user).map_err(|e| refused(format!("[DESIGN] {}", e.msg)))?;
    let authenticator =
        services.authenticator(mechanism).ok_or_else(|| refused(format!("[DESIGN] mechanism {mechanism} is not served")))?;
    Ok((user, authenticator))
}

/// The authorization identifier `exchange` accepts, given the client's
/// `initial` bytes and its answers to the challenges sent it: each step
/// checked apart from the workers, the client's address having waited its
/// turn after failed logins (`authenticate`), its failure counted
/// (`logins.rs`).
async fn exchanged(
    wire: &mut Wire,
    (peer, present): (&Peer, &Present),
    services: &Services,
    (begun, initial): (Box<dyn Exchange>, Vec<u8>),
) -> Result<String> {
    let (logins, db) = (services.logins(), services.database());
    let (mut exchange, mut input) = (begun, initial);
    loop {
        let (taken, step) = logins
            .check(db, move || {
                let step = exchange.step(&input);
                (exchange, step)
            })
            .await?;
        exchange = taken;
        match step {
            Step::Challenge(challenge) => wire.send_now(&Reply::Challenge(challenge)).await?,
            Step::Accept(authorization) => {
                logins.accepted(peer);
                return Ok(authorization);
            }
            Step::Refuse(why) => {
                logins.failed(db, peer);
                return Err(refused(why));
            }
        }
        let Request::Answer(answer) = next(wire, services, present).await? else {
            return Err(broken("[DESIGN] a challenge is answered with Answer"));
        };
        input = answer;
    }
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

/// The work of the client's next request, none for Terminate: waited for
/// SERVER.IDLE_TIMEOUT_MS, or SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS too if
/// `in_transaction`. Refused (SQLSTATE 28000) if the session is `user`'s,
/// and the user has been dropped since it logged in.
async fn next_work(
    wire: &mut Wire,
    (services, present): (&Services, &Present),
    in_transaction: bool,
    user: Option<&str>,
) -> Result<Option<Work>> {
    let waiting = next(wire, services, present);
    let request = if in_transaction { services.idle_in_transaction(waiting).await } else { services.idle(waiting).await };
    let work = work_of(request?)?;
    let Some(name) = user else { return Ok(work) };
    if services.database().is_user(name)? {
        return Ok(work);
    }
    Err(refused(format!("[DESIGN] the user {name} was dropped: its connections end")))
}

/// Runs the client's requests in `admitted`'s session until it terminates,
/// the connection fails or the server stops; then ends the session on a
/// worker: as SESSION.COMMIT_AT_END says if the client terminated it, its failure
/// returned to be sent before the connection closes; else rolled back
/// [ISO/IEC 9075:1992 §20.1 GR2a]. Shown running while a request is.
async fn run(wire: &mut Wire, (services, present): (&Services, &Present), admitted: Admitted) -> Result<()> {
    // The key cancels the session's statements while it lives.
    let Admitted { session, key: _key, user } = admitted;
    let mut held = Some(Held::new(session));
    let ended = loop {
        let in_transaction = held.as_ref().is_some_and(|h| h.session.transaction().is_some());
        let work = match next_work(wire, (services, present), in_transaction, user.as_deref()).await {
            Ok(Some(work)) => work,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        let Some(lent) = held.take() else { break Err(broken("[DESIGN] the session is gone")) };
        present.running(true);
        let exchanged = exchange(wire, (services, present), lent, work).await;
        present.running(false);
        match exchanged {
            Ok(back) => held = Some(back),
            Err(e) => break Err(e),
        }
    };
    let Some(back) = held else { return ended };
    match ended {
        Ok(()) => terminated(services, back).await,
        Err(e) => {
            end(services, back).await?;
            Err(e)
        }
    }
}

/// Ends `held`'s session on a worker as SESSION.COMMIT_AT_END says, its client
/// having terminated it [ISO/IEC 9075:1992 §20.1 GR2a]: a commit that
/// fails, as a deferred constraint not satisfied, is the client's error.
async fn terminated(services: &Services, held: Held) -> Result<()> {
    services.run(move || held.session.end()).await?
}

/// Ends `held`'s session on a worker, rolled back: it ended unexpectedly
/// [ISO/IEC 9075:1992 §20.1 GR2a].
async fn end(services: &Services, held: Held) -> Result<()> {
    services
        .run(move || {
            // A failed rollback has marked its tables damaged (RULES.md
            // D19): nothing is left for the client.
            let _ = held.session.rollback();
        })
        .await
}

/// Does `work` with `lent` on a worker, sending its replies as they come,
/// until the operator ends the connection: what it holds, given back.
async fn exchange(wire: &mut Wire, (services, present): (&Services, &Present), lent: Held, work: Work) -> Result<Held> {
    let (sender, mut receiver) = mpsc::channel(IN_FLIGHT);
    let db = services.database();
    let (batch, max_statements) = (settings::count(db, settings::BATCH_BYTES), settings::count(db, settings::MAX_STATEMENTS));
    let mut held = lent;
    let worked = services.run(move || {
        held.handle(work, (&sender, batch), max_statements);
        held
    });
    let running = stopped(worked, present);
    let forwarding = async move {
        while let Some(reply) = receiver.recv().await {
            // A client that stops taking what it is sent holds a worker:
            // it is given SERVER.IDLE_TIMEOUT_MS, as one sending nothing is.
            services.idle(wire.send(&reply)).await?;
            if receiver.is_empty() {
                services.idle(wire.flush()).await?;
            }
        }
        Ok::<(), SqlError>(())
    };
    // Ended, the replies are let go, so a worker sending them stops: what
    // it ran is cancelled (`Operator::end`).
    let forward = async {
        tokio::select! {
            forwarded = forwarding => forwarded,
            () = present.ended() => Err(ended()),
        }
    };
    let (back, forwarded) = tokio::join!(running, forward);
    let back = back?;
    if let Err(e) = forwarded {
        // As `run`'s: the connection is lost.
        end(services, back).await?;
        return Err(e);
    }
    Ok(back)
}
