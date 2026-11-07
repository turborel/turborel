//! \[DESIGN\] The native protocol's server side (RULES.md D42, D45; the
//! messages in `turborel_protocol`): a connection authenticates, then runs
//! its requests in its session, one at a time, in order; or, its first and
//! only message a Cancel, cancels the statement another connection runs.
//!
//! A request runs on a worker, lent the session, its prepared statements
//! and their open cursors, and the writer of the connection's replies,
//! while it runs (`native/requests.rs`, `native/lent.rs`). It writes its
//! replies itself, a batch at a time, each within SERVER.IDLE_TIMEOUT_MS,
//! so a client reading slowly holds the worker but never more than a batch
//! of memory, and a client gone fails a write, so the worker stops
//! fetching. The connection's task reads the next request meanwhile, and
//! waits for the loan back only to run it. The session ends on a worker too: as
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

mod lent;
mod requests;
mod wire;

use std::future::Future;
use std::sync::Arc;

use lent::{Lent, Loan, Sending, Slot};
use requests::{Held, Work};
use turborel::db::Session;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::value::types::TypeRegistry;
use turborel_protocol::{Reply, Request, VERSION, broken};
use wire::{Wire, Writer};

use crate::auth::{Authenticator, Exchange, Peer, Step};
use crate::server::{Connection, Protocol, Registered, Services, Serving};
use crate::settings;
use crate::shown::Present;
use crate::told;
use crate::transport::Transport;

/// \[DESIGN\] How often a statement of a connection its operator ended is
/// cancelled again until it stops: a cancel made before it began does
/// nothing (RULES.md D44). As often as a wait for a lock looks.
const CANCEL_AGAIN: std::time::Duration = std::time::Duration::from_millis(20);
/// \[DESIGN\] How long a refused connection is held after its refusal,
/// for the client to close it (`Wire::close_refusing`): one that has read
/// its refusal closes at once; one that neither sends nor closes holds a
/// refusal's place (SERVER.MAX_REFUSALS) this long.
const REFUSED_WITHIN: std::time::Duration = std::time::Duration::from_millis(500);

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
            let (mut wire, mut writer) = Wire::new(transport, LOGIN_MESSAGE_BYTES, TypeRegistry::default());
            wire::close_refusing(&mut wire, &mut writer, &why, REFUSED_WITHIN).await;
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
    let (mut wire, mut writer) = Wire::new(transport, max_body.min(LOGIN_MESSAGE_BYTES), types);
    let (mut writer, ended) = match services.login(arrived, welcome(&mut wire, &mut writer, (&peer, &present), &services)).await {
        Ok(Some(admitted)) => {
            wire.set_max_body(max_body);
            run(&mut wire, writer, (&services, &present), admitted).await
        }
        Ok(None) => (writer, Ok(())),
        Err(e) => (writer, Err(e)),
    };
    // Told within SERVER.IDLE_TIMEOUT_MS, as any reply: a client that takes
    // nothing would hold the connection open.
    let closing = async {
        match ended {
            Ok(()) => writer.close().await,
            Err(e) => writer.close_with(&e).await,
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
async fn welcome(
    wire: &mut Wire,
    writer: &mut Writer,
    (peer, present): (&Peer, &Present),
    services: &Services,
) -> Result<Option<Admitted>> {
    match next(wire, services, present).await? {
        Request::Cancel(key) => {
            services.cancel(&key);
            Ok(None)
        }
        // No capability is defined in version 1, so none is taken.
        Request::Hello { version, user, mechanism, initial, capabilities: _ } => {
            authenticate((wire, writer), (peer, present), services, (version, user, mechanism, initial)).await.map(Some)
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
    (wire, writer): (&mut Wire, &mut Writer),
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
    let authorization = exchanged((wire, writer), (peer, present), services, (exchange, initial)).await?;
    // A user's session lasts while the user does (RULES.md D43).
    let member = db.is_user(&authorization)?.then(|| authorization.clone());
    let session = db.session(&authorization);
    let key = services.register_cancel(session.canceller())?;
    present.stoppable(session.canceller());
    present.authenticated(&authorization);
    writer.send_now(&Reply::Ready { authorization, key: key.key, capabilities: Vec::new(), server: turborel::product() }).await?;
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
    (wire, writer): (&mut Wire, &mut Writer),
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
            Step::Challenge(challenge) => writer.send_now(&Reply::Challenge(challenge)).await?,
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
/// SERVER.IDLE_TIMEOUT_MS, or SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS too while
/// the session's transaction is active, as `slot` says (`awaited`).
/// Refused (SQLSTATE 28000) if the session is `user`'s, and the user has
/// been dropped since it logged in.
async fn next_work(wire: &mut Wire, (services, present): (&Services, &Present), slot: &Slot, user: Option<&str>) -> Result<Option<Work>> {
    let request = awaited(services, slot, next(wire, services, present)).await;
    let work = work_of(request?)?;
    let Some(name) = user else { return Ok(work) };
    if services.database().is_user(name)? {
        return Ok(work);
    }
    Err(refused(format!("[DESIGN] the user {name} was dropped: its connections end")))
}

/// `work`, done within SERVER.IDLE_TIMEOUT_MS, and within
/// SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS too while the session's transaction
/// is active, as `slot` last saw it. The worker answering the request
/// before may begin or end the transaction meanwhile: the limit is chosen
/// again when it does, `work` going on.
async fn awaited<T>(services: &Services, slot: &Slot, work: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::pin!(work);
    loop {
        // Enabled before the transaction is looked at, so a change between
        // the two is not missed.
        let changed = slot.changed();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let bounded = async { if slot.active() { services.idle_in_transaction(&mut work).await } else { services.idle(&mut work).await } };
        tokio::select! {
            biased;
            done = bounded => return done,
            () = changed => {}
        }
    }
}

/// SERVER.BATCH_BYTES and SERVER.MAX_STATEMENTS, as they are now.
fn limits(services: &Services) -> (usize, usize) {
    let db = services.database();
    (settings::count(db, settings::BATCH_BYTES), settings::count(db, settings::MAX_STATEMENTS))
}

/// Runs the client's requests in `admitted`'s session until it terminates,
/// the connection fails or the server stops; then ends the session on a
/// worker: as SESSION.COMMIT_AT_END says if the client terminated it, its failure
/// returned to be sent before the connection closes; else rolled back
/// [ISO/IEC 9075:1992 §20.1 GR2a]. Each request runs on a worker lent the
/// session and `writer` (`native/lent.rs`), shown running meanwhile; the
/// writer comes back with the outcome, for the connection to close with.
async fn run(wire: &mut Wire, writer: Writer, (services, present): (&Services, &Present), admitted: Admitted) -> (Writer, Result<()>) {
    // The key cancels the session's statements while it lives.
    let Admitted { session, key: _key, user } = admitted;
    let slot = Slot::new(Held::new(session), writer);
    let sending = Sending::new(services, present);
    let mut back = None;
    let ended = loop {
        let work = match next_work(wire, (services, present), &slot, user.as_deref()).await {
            Ok(Some(work)) => work,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        // Given back once the request before is answered: at once, unless
        // the client sent this one with it. Until then, an operator ending
        // the connection cancels what runs.
        let mut lent = stopped(slot.take(), present).await;
        if let Some(failed) = lent.failed.take() {
            back = Some(lent);
            break Err(failed);
        }
        let ending = wire.arrived(|request| work_of(request).ok().flatten().filter(|next| lent.held.ends_transaction(next)));
        let then = ending.map(|next| (next, still_served(services, present, user.as_deref())));
        let (loan, sending, limits) = (Loan::new(&slot, lent), sending.clone(), limits(services));
        // Not waited for: the loan comes back once the replies are written.
        drop(services.spawn(move || loan.run((work, then), &sending, limits)).await);
    };
    let Lent { held, writer: returned, failed } = match back {
        Some(lent) => lent,
        None => stopped(slot.take(), present).await,
    };
    // A write that failed lost the connection, whatever came after.
    let ended = match failed.map_or(ended, Err) {
        Ok(()) => terminated(services, held).await,
        Err(e) => end(services, held).await.and(Err(e)),
    };
    (returned, ended)
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

/// What says whether the connection is still served, as `next` and
/// `next_work` ask before each request: asked again on the worker, before
/// the request sent with the one it runs.
fn still_served(services: &Services, present: &Present, user: Option<&str>) -> impl FnOnce() -> bool + Send + 'static {
    let (serving, ended, db, user) = (services.serving(), present.watching(), services.database().clone(), user.map(str::to_owned));
    move || serving() && !*ended.borrow() && user.is_none_or(|name| db.is_user(&name).unwrap_or(false))
}
