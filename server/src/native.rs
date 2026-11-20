//! \[DESIGN\] The native protocol's server side (RULES.md D42, D45; the
//! messages in `turborel_protocol`): a connection authenticates, then runs
//! its requests in its session, one at a time, in order; or, its first and
//! only message a Cancel, cancels the statement another connection runs.
//!
//! A request runs on a worker, lent the session, its prepared statements
//! and their open cursors, and the wire of the connection's messages,
//! while it runs (`native/requests.rs`, `native/lent.rs`). It writes its
//! replies itself, a batch at a time, each within SERVER.IDLE_TIMEOUT_MS,
//! so a client reading slowly holds the worker but never more than a batch
//! of memory, and a client gone fails a write, so the worker stops
//! fetching. A fast client's next request the worker reads itself, and
//! runs: it waits a moment for one (`lent::WAITED`), apart from the
//! runtime, its permit to run statements given up meanwhile. A request
//! that comes later, the connection's task reads through the runtime, and
//! lends the session and wire again. The session ends on a worker too: as
//! SESSION.COMMIT_AT_END says once the client terminates it, a commit that
//! fails sent as the client's error before the connection closes; rolled
//! back if the connection ends otherwise, lost or failed, cut by the
//! server stopping or ended by its operator, as a direct invocation of SQL
//! that terminated unexpectedly [ISO/IEC 9075:1992 §20.1 GR2a]. A client
//! gone while its request runs, its socket closed or failed, is found by
//! a watch on its socket (`transport/watch.rs`, RULES.md D335), and what
//! runs cancelled as an operator's ending cancels it, so its locks go.
//!
//! A connection has SERVER.LOGIN_TIMEOUT_MS to authenticate, in messages of
//! at most `LOGIN_MESSAGE_BYTES`, its checks run apart from the workers
//! and slowed after failures from its address (`logins.rs`). It is closed,
//! its session rolled back, once it has kept the server waiting
//! SERVER.IDLE_TIMEOUT_MS for a request, or to take what it is sent, or
//! SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS for a request while its
//! transaction is active. A session of a user is ended at its next request
//! once the user is dropped (RULES.md D43).

mod copy;
mod lent;
mod lobs;
mod next;
mod notices;
mod position;
mod renew;
mod requests;
mod status;
mod wire;

use std::future::Future;
use std::sync::Arc;

use lent::{Lent, Loan, Running, Slot};
use next::{Next, next_work};
use requests::{Held, Serves};
use turborel::db::Session;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::value::types::TypeRegistry;
use turborel_protocol::capability::{COMPRESSION, DIAGNOSTICS, STATUS};
use turborel_protocol::{Attributes, Capabilities, Reply, Request, VERSION, broken};
use wire::Wire;

use crate::auth::{Authenticator, Exchange, Peer, Step};
use crate::server::{Connection, Protocol, Registered, Services, Serving};
use crate::settings;
use crate::shown::Present;
use crate::told;
use crate::transport::{Transport, Watch};

/// \[DESIGN\] How often a statement of a connection its operator ended is
/// cancelled again until it stops: a cancel made before it began does
/// nothing (RULES.md D44). As often as a wait for a lock looks.
const CANCEL_AGAIN: std::time::Duration = std::time::Duration::from_millis(20);
/// \[DESIGN\] How often a connection whose request runs is looked at for
/// its client gone (RULES.md D335). A request answered sooner is never
/// looked at.
const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(100);
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
            let mut wire = Wire::new(transport, LOGIN_MESSAGE_BYTES, TypeRegistry::default());
            wire.close_refusing(&why, REFUSED_WITHIN).await;
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
/// connection first, or `watch` finds its client gone (RULES.md D335),
/// what its session runs cancelled, again every `CANCEL_AGAIN` until it
/// stops.
async fn stopped<T>(worked: impl Future<Output = T>, present: &Present, watch: Option<Watch>) -> T {
    tokio::pin!(worked);
    let gone = async {
        match watch {
            Some(watch) => watch.gone_within(LOOK_AGAIN).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        back = &mut worked => return back,
        () = present.ended() => {}
        () = gone => {}
    }
    loop {
        present.cancel();
        if let Ok(back) = tokio::time::timeout(CANCEL_AGAIN, &mut worked).await {
            return back;
        }
    }
}

/// Who a connection's session is of: its cancel key, registered while
/// this lives, the user it logged in as, if that was a user, and its
/// authorization identifier.
struct Login {
    key: Registered,
    user: Option<String>,
    authorization: String,
}

/// A client authenticated: its session, its login, and what it was
/// granted (RULES.md D185).
struct Admitted {
    session: Session,
    login: Login,
    granted: Capabilities,
    /// What the login told of the session's settings, held so a statement
    /// sends only what it changes.
    told: status::Status,
}

/// What a Hello offers beside its credentials: the capabilities it asks
/// for and the connection's attributes (RULES.md D185, D191).
struct Offer {
    capabilities: Vec<String>,
    attributes: Attributes,
}

/// Serves `connection` to its end.
async fn serve(connection: Connection) {
    let Connection { transport, peer, services, present, arrived } = connection;
    let max_body = u32::try_from(settings::count(services.database(), settings::MAX_MESSAGE_BYTES)).unwrap_or(u32::MAX);
    // A client's values of types beyond the standard read as the
    // database's (RULES.md D53).
    let types = services.database().types().unwrap_or_default();
    let mut wire = Wire::new(transport, max_body.min(LOGIN_MESSAGE_BYTES), types);
    let (mut wire, ended) = match services.login(arrived, welcome(&mut wire, (&peer, &present), &services)).await {
        Ok(Some(admitted)) => {
            wire.set_max_body(max_body);
            Box::pin(run(wire, (&services, &present, &peer), admitted)).await
        }
        Ok(None) => (wire, Ok(())),
        Err(e) => (wire, Err(e)),
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
        Request::Hello { version, user, mechanism, initial, capabilities, attributes } => {
            let offer = Offer { capabilities, attributes };
            authenticate(wire, (peer, present), services, ((version, user, mechanism, initial), offer)).await.map(Some)
        }
        Request::Answer(_)
        | Request::Query { .. }
        | Request::Prepare { .. }
        | Request::Execute { .. }
        | Request::Fetch { .. }
        | Request::Close { .. }
        | Request::Deallocate { .. }
        | Request::Lob(_)
        | Request::Describe(_)
        | Request::Ping
        | Request::Reset
        | Request::ChangeUser { .. }
        | Request::CopyIn { .. }
        | Request::CopyData(_)
        | Request::CopyEnd { .. }
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
    (hello, offer): ((u32, String, String, Vec<u8>), Offer),
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
    not_a_role(services, &authorization)?;
    // A user's session lasts while the user does (RULES.md D43).
    let member = db.is_user(&authorization)?.then(|| authorization.clone());
    let session = db.session(&authorization);
    let key = services.register_cancel(session.canceller())?;
    present.stoppable(session.canceller());
    present.authenticated(&authorization);
    present.attributed(offer.attributes);
    let threshold = settings::count(db, settings::COMPRESSION_MIN_BYTES);
    let offered = if threshold == 0 { Capabilities::all().without(COMPRESSION) } else { Capabilities::all() };
    let granted = Capabilities::of(&offer.capabilities).and(offered);
    let mut told = status::Status::default();
    let mut replies = if granted.has(STATUS) { told.begun(&session) } else { Vec::new() };
    let ready =
        Reply::Ready { authorization: authorization.clone(), key: key.key, capabilities: granted.names(), server: turborel::product() };
    replies.push(ready);
    wire.send_all(&replies).await?;
    wire.grant(granted.has(COMPRESSION).then_some(threshold), granted.has(DIAGNOSTICS));
    Ok(Admitted { session, login: Login { key, user: member, authorization }, granted, told })
}

/// Refuses `authorization` as a session's user (SQLSTATE 28000) if it is
/// a role's name.
fn not_a_role(services: &Services, authorization: &str) -> Result<()> {
    if services.database().is_role(authorization)? {
        let msg = format!("[ISO/IEC 9075-2:2023 §19.2 GR5] {authorization} is a role's name, and a session's user is a user's");
        return Err(refused(msg));
    }
    Ok(())
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

/// SERVER.BATCH_BYTES, SERVER.MAX_STATEMENTS and SERVER.LOB_INLINE_BYTES,
/// as they are now.
fn limits(services: &Services) -> (usize, usize, usize) {
    let db = services.database();
    (
        settings::count(db, settings::BATCH_BYTES),
        settings::count(db, settings::MAX_STATEMENTS),
        settings::count(db, settings::LOB_INLINE_BYTES),
    )
}

/// Runs the client's requests in `admitted`'s session until it terminates,
/// the connection fails or the server stops; then ends the session on a
/// worker: as SESSION.COMMIT_AT_END says if the client terminated it, its
/// failure returned to be sent before the connection closes; else rolled
/// back [ISO/IEC 9075:1992 §20.1 GR2a]. Each request read here runs on a
/// worker lent the session and `wire` (`native/lent.rs`), which answers a
/// fast client's next requests too, shown running meanwhile; the wire
/// comes back with the outcome, for the connection to close with. A Reset
/// or a ChangeUser renews the session here (`native/renew.rs`).
async fn run(wire: Wire, (services, present, peer): (&Services, &Present, &Peer), admitted: Admitted) -> (Wire, Result<()>) {
    let Admitted { session, mut login, granted, told } = admitted;
    let slot = Slot::new();
    let mut running = Running::new(services, present, login.user.as_deref());
    let serves = Serves { db: services.database().clone(), requests: Arc::clone(services.requests()) };
    let mut fresh = Held::new(session, serves, granted);
    fresh.status = told;
    let mut lent = Lent::new(fresh, wire);
    let ended = loop {
        let work = match next_work(&mut lent, (services, present), login.user.as_deref()).await {
            Ok(Next::Work(work)) => work,
            Ok(Next::Terminate) => break Ok(()),
            Ok(Next::Reset) => match renew::reset(&mut lent, (services, present), &login).await {
                Ok(()) => continue,
                Err(e) => break Err(e),
            },
            Ok(Next::ChangeUser { user, mechanism, initial }) => {
                let changed = renew::change_user(&mut lent, (services, present, peer), &mut login, (user, mechanism, initial)).await;
                running = Running::new(services, present, login.user.as_deref());
                match changed {
                    Ok(()) => continue,
                    Err(e) => break Err(e),
                }
            }
            Err(e) => break Err(e),
        };
        let watch = lent.wire.watch();
        let (loan, running, limits) = (Loan::new(&slot, lent), running.clone(), limits(services));
        // Not waited for: the loan comes back once the request is answered,
        // and the fast client's requests after it (`Loan::run`). Until
        // then, an operator ending the connection, or its client gone,
        // cancels what runs.
        drop(services.spawn_with(move |permit| loan.run(work, &running, permit, limits)).await);
        lent = stopped(slot.take(), present, watch).await;
        // A read or a write that failed lost the connection.
        if let Some(failed) = lent.failed.take() {
            break Err(failed);
        }
    };
    let Lent { held, wire: returned, .. } = lent;
    let ended = match ended {
        Ok(()) => terminated(services, held).await,
        Err(e) => end(services, held).await.and(Err(e)),
    };
    (returned, ended)
}

/// Ends `held`'s session on a worker as SESSION.COMMIT_AT_END says, its client
/// having terminated it [ISO/IEC 9075:1992 §20.1 GR2a]: a commit that
/// fails, as a deferred constraint not satisfied, is the client's error.
async fn terminated(services: &Services, mut held: Held) -> Result<()> {
    services
        .run(move || {
            held.abandon_copy();
            held.session.end()
        })
        .await?
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
