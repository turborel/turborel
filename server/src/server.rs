//! \[DESIGN\] The server (RULES.md D42): listeners take connections, and a
//! `Protocol` plug-in serves each, on the services the server shares:
//! the database, its authenticators, and a bounded pool of workers that
//! run statements. Threads doing I/O never run one: a statement may wait
//! for a lock, a page or a sync, so it runs on a worker, and the workers'
//! number caps how many run at once.
//!
//! Its limits are the database's settings (`settings.rs`), read as they
//! are needed, so a change takes effect without a restart.
//!
//! The server stops when told: no connection is taken after that, each
//! connection finishes the statement it runs and is closed, and `serve`
//! returns once every one is; the database is then the caller's, to close.
//! Past SERVER.SHUTDOWN_TIMEOUT_MS, or told to stop again, it hurries: the
//! statements its sessions run are cancelled (RULES.md D44), and every
//! wait on a client ends, so each connection is closed, its session rolled
//! back.
//!
//! It tells what it does (`told.rs`): where it listens, that it stops or
//! hurries, accepts that failed, TLS and logins that failed, and
//! connections refused past SERVER.MAX_CONNECTIONS.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio::task::JoinSet;
use turborel::db::Database;
use turborel::diagnostics::Event;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::txn::Stop;
use turborel_protocol::{CancelKey, KEY_BYTES, broken};

use crate::auth::{Authenticator, Peer};
use crate::holding::{Hold, Holding};
use crate::limit::{Held, Limit};
use crate::logins::Logins;
use crate::password::Pbkdf2;
use crate::settings::{
    IDLE_IN_TRANSACTION_TIMEOUT_MS, IDLE_TIMEOUT_MS, LOGIN_TIMEOUT_MS, MAX_CONNECTIONS, MAX_REFUSALS, SHUTDOWN_TIMEOUT_MS, WORKERS,
};
use crate::settings::{count, declare, millis};
use crate::shown::{self, Operator, Present, Shown};
use crate::tls::{Handshaken, Secured, Tls};
use crate::told::{Throttle, Told};
use crate::transport::{Listener, Transport};

/// \[DESIGN\] How long a listener waits after failing to accept, so a
/// failure that lasts, such as running out of file descriptors, does not
/// keep a thread busy. Fixed, not a setting (RULES.md D36): it answers a
/// failure of the system's, which no work of the server's tunes.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// \[DESIGN\] How often a server hurrying cancels its sessions' statements
/// again, so one begun as it hurried is cancelled too.
const RECANCEL: Duration = Duration::from_millis(100);

/// How far a server has come to stopping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Taking connections.
    Serving,
    /// Told to stop: each connection finishes its statement.
    Stopping,
    /// Past SERVER.SHUTDOWN_TIMEOUT_MS, or told again: statements cancelled,
    /// waits on clients ended.
    Hurried,
}

/// A connection's place: under SERVER.MAX_CONNECTIONS, served; or among
/// the refusals in flight, refused.
enum Place {
    Served(Held),
    Refused(Held),
}

/// The place of a connection taken: under `most` of `connections`, else
/// among `refusing` refusals in flight (SERVER.MAX_REFUSALS); none past
/// these.
fn place(connections: &Arc<Limit>, refusals: &Arc<Limit>, (most, refusing): (usize, usize)) -> Option<Place> {
    connections.try_take(most).map(Place::Served).or_else(|| refusals.try_take(refusing).map(Place::Refused))
}

/// What a `Protocol` returns.
pub type Serving = Pin<Box<dyn Future<Output = ()> + Send>>;

/// \[DESIGN\] A wire protocol: an extension point (RULES.md D39, D42), so
/// more than one can serve the same database.
///
/// Contract (RULES.md R-SAFE-3): `serve` runs a connection to its end:
/// authenticates it through the services' authenticators, each check on
/// a login's thread (`Services::logins`) within SERVER.LOGIN_TIMEOUT_MS of
/// `Connection::arrived` (`Services::login`), runs each statement on a
/// worker (`Services::run`), and ends when the client goes, the server
/// stops (`Services::stopping`) or its operator ends it
/// (`Present::ended`), its session ended on a worker. `refuse` tells the client why it is refused, and closes.
pub trait Protocol: Send + Sync {
    /// The protocol's name.
    fn name(&self) -> &'static str;
    /// Serves `connection` to its end.
    fn serve(&self, connection: Connection) -> Serving;
    /// Refuses `transport`'s client for `why`.
    fn refuse(&self, transport: Box<dyn Transport>, why: SqlError) -> Serving;
}

/// A connection taken, for a `Protocol` to serve.
pub struct Connection {
    /// Its stream.
    pub transport: Box<dyn Transport>,
    /// Who it came from.
    pub peer: Peer,
    /// What the server shares.
    pub services: Services,
    /// Its row of INFORMATION_SCHEMA.CONNECTIONS, for what serves it to
    /// say who logged in and whether a statement runs (`shown.rs`).
    pub present: Present,
    /// When it was taken: its SERVER.LOGIN_TIMEOUT_MS runs from then, TLS's
    /// handshake and authentication within the one limit
    /// (`Services::login`).
    pub arrived: Instant,
}

/// A listener, and the protocol it serves.
pub struct Endpoint {
    /// Where connections come from.
    pub listener: Box<dyn Listener>,
    /// What serves them.
    pub protocol: Arc<dyn Protocol>,
    /// The TLS that secures them, if any.
    pub tls: Option<Arc<Tls>>,
}

/// \[DESIGN\] What a server shares with the connections it serves.
#[derive(Clone)]
pub struct Services(Arc<Shared>);

struct Shared {
    db: Database,
    authenticators: Vec<Arc<dyn Authenticator>>,
    workers: Arc<Limit>,
    stop: watch::Receiver<Stage>,
    /// What cancels each connection's statements, by its key (RULES.md D45).
    cancels: Mutex<HashMap<CancelKey, Arc<Stop>>>,
    /// What it shows of itself (`shown.rs`).
    shown: Arc<Shown>,
    /// Failed accepts, and connections refused, told at most once a minute.
    throttles: (Throttle, Throttle),
    /// What checks logins, apart from the workers (`logins.rs`).
    logins: Logins,
}

/// \[DESIGN\] A connection's cancel key, registered until this is dropped.
pub struct Registered {
    services: Services,
    /// The key.
    pub key: CancelKey,
}

impl Drop for Registered {
    fn drop(&mut self) {
        lock_builtin(&self.services.0.cancels).remove(&self.key);
    }
}

impl Services {
    /// The database served.
    pub fn database(&self) -> &Database {
        &self.0.db
    }

    /// The authenticator of `mechanism`, if the server has one: its
    /// refusals told, for an audit of who tried to connect.
    pub fn authenticator(&self, mechanism: &str) -> Option<Arc<dyn Authenticator>> {
        let inner = self.0.authenticators.iter().find(|a| a.mechanism() == mechanism).cloned()?;
        Some(Arc::new(Told { inner, db: self.0.db.clone() }))
    }

    /// Runs `work` on a worker, once one is free: once fewer statements
    /// run than SERVER.WORKERS says.
    ///
    /// # Errors
    ///
    /// A connection failure if the worker failed, which is a defect.
    pub async fn run<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
        let waiting = Counted::of(&self.0.shown, |s| &s.waiting);
        let permit = self.0.workers.take(|| count(&self.0.db, WORKERS)).await;
        drop(waiting);
        let busy = Counted::of(&self.0.shown, |s| &s.busy);
        let ran = tokio::task::spawn_blocking(move || {
            let _held = (permit, busy);
            work()
        });
        ran.await.map_err(|_| broken("[DESIGN] a worker failed"))
    }

    /// What checks logins, on threads apart from the workers, and slows
    /// clients whose logins fail (`logins.rs`).
    pub fn logins(&self) -> &Logins {
        &self.0.logins
    }

    /// `work`, if it is done within SERVER.LOGIN_TIMEOUT_MS of `arrived`, when
    /// the connection was taken: as it is secured and authenticated, the
    /// two within the one limit, so a client that never finishes does not
    /// hold its place.
    ///
    /// # Errors
    ///
    /// `work`'s; a connection failure if the time ran out.
    pub async fn login<T>(&self, arrived: Instant, work: impl Future<Output = Result<T>>) -> Result<T> {
        self.bounded(LOGIN_TIMEOUT_MS, arrived, work, "to begin TLS and authenticate").await
    }

    /// `work`, if it is done within SERVER.IDLE_TIMEOUT_MS: as the server waits
    /// on its client, for a request or to take what it is sent.
    ///
    /// # Errors
    ///
    /// `work`'s; a connection failure if the time ran out.
    pub async fn idle<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        self.bounded(IDLE_TIMEOUT_MS, Instant::now(), work, "with nothing from or to the client").await
    }

    /// `work`, if it is done within the milliseconds of the setting `name`
    /// of `from`; none of 0, no limit; and before the server hurries. The
    /// connection fails once the time runs out, having waited `waiting`, or
    /// the server hurries.
    async fn bounded<T>(&self, name: &str, from: Instant, work: impl Future<Output = Result<T>>, waiting: &str) -> Result<T> {
        let timed = async {
            let Some(limit) = millis(self.database(), name) else { return work.await };
            tokio::time::timeout(limit.saturating_sub(from.elapsed()), work).await.unwrap_or_else(|_| {
                Err(broken(format!("[DESIGN] the connection went past {name}, {} ms, {waiting}: closed", limit.as_millis())))
            })
        };
        tokio::select! {
            done = timed => done,
            () = self.hurried() => Err(broken("[DESIGN] the server is stopping, past SERVER.SHUTDOWN_TIMEOUT_MS or told again: closed")),
        }
    }

    /// `work`, if it is done within SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS
    /// and SERVER.IDLE_TIMEOUT_MS, the shorter: as the server waits for the
    /// next request of a client whose transaction is active, its locks
    /// held.
    ///
    /// # Errors
    ///
    /// `work`'s; a connection failure if the time ran out.
    pub async fn idle_in_transaction<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        let waiting = "with its transaction active and nothing from the client";
        self.idle(self.bounded(IDLE_IN_TRANSACTION_TIMEOUT_MS, Instant::now(), work, waiting)).await
    }

    /// Done once the server stops.
    pub async fn stopping(&self) {
        let mut stop = self.0.stop.clone();
        // An error means the server is gone: stopped, too.
        let _ = stop.wait_for(|stage| *stage != Stage::Serving).await;
    }

    /// Done once the server hurries.
    async fn hurried(&self) {
        let mut stop = self.0.stop.clone();
        let _ = stop.wait_for(|stage| *stage == Stage::Hurried).await;
    }

    /// Cancels the statement every session runs: the sessions'.
    fn cancel_all(&self) -> u64 {
        let cancels = lock_builtin(&self.0.cancels);
        cancels.values().for_each(|stop| stop.cancel());
        u64::try_from(cancels.len()).unwrap_or(u64::MAX)
    }

    /// One more connection refused past SERVER.MAX_CONNECTIONS: told at
    /// most once a minute.
    fn refused(&self) {
        if let Some(refused) = self.0.throttles.1.one(Instant::now()) {
            let limit = u64::try_from(count(self.database(), MAX_CONNECTIONS)).unwrap_or(u64::MAX);
            self.database().tell(&Event::ConnectionsRefused { refused, limit });
        }
    }

    /// One more accept that failed, for `why`: told at most once a minute.
    fn accept_failed(&self, why: &std::io::Error) {
        if let Some(failures) = self.0.throttles.0.one(Instant::now()) {
            self.database().tell(&Event::AcceptFailed { failures, message: why.to_string() });
        }
    }

    /// \[DESIGN\] A new key, of the system's randomness, that cancels
    /// `stop`'s statements from another connection (RULES.md D45); kept
    /// until what is returned is dropped.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the system gives no randomness.
    pub fn register_cancel(&self, stop: Arc<Stop>) -> Result<Registered> {
        let mut key = [0_u8; KEY_BYTES];
        aws_lc_rs::rand::fill(&mut key).map_err(|_| SqlError::new(Code::Io, "[DESIGN] the system gave no randomness for a key"))?;
        let key = CancelKey(key);
        lock_builtin(&self.0.cancels).insert(key, stop);
        Ok(Registered { services: self.clone(), key })
    }

    /// \[DESIGN\] Cancels the statement the session of `key` runs, if it
    /// is one's (RULES.md D45).
    pub fn cancel(&self, key: &CancelKey) {
        if let Some(stop) = lock_builtin(&self.0.cancels).get(key) {
            stop.cancel();
        }
    }
}

/// A lock whose holder cannot leave it broken: what it guards is read or
/// replaced whole.
fn lock_builtin<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// \[DESIGN\] A server of `Database`: the database stays the caller's,
/// who may run sessions of its own beside the server's.
pub struct Server {
    db: Database,
    authenticators: Vec<Arc<dyn Authenticator>>,
    shown: Arc<Shown>,
}

/// One more of a count of `shown.rs`, while it is held.
struct Counted(Arc<Shown>, fn(&Shown) -> &AtomicU64);

impl Counted {
    fn of(shown: &Arc<Shown>, which: fn(&Shown) -> &AtomicU64) -> Counted {
        which(shown).fetch_add(1, Ordering::Relaxed);
        Counted(Arc::clone(shown), which)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        (self.1)(&self.0).fetch_sub(1, Ordering::Relaxed);
    }
}

impl Server {
    /// A server of `db`, with no authenticator yet; the server's settings
    /// declared on `db`, unless they are already, and its passwords'
    /// verifiers made as they say (`password::Pbkdf2`), for the users SQL
    /// creates; and its tables, CONNECTIONS and SERVER, and what END and
    /// CANCEL CONNECTION reach its connections by (`Operator`), registered
    /// with it (`shown.rs`), in place of any of a server before.
    ///
    /// # Errors
    ///
    /// `settings::declare`'s; those of registering a system table or
    /// setting a plug-in.
    pub fn new(db: Database) -> Result<Server> {
        declare(&db)?;
        db.set_credentials(Box::new(Pbkdf2::of(&db)))?;
        let shown = Arc::new(Shown::default());
        db.register_system_table(Box::new(shown::Connections(Arc::clone(&shown))))?;
        db.register_system_table(Box::new(shown::Server(Arc::clone(&shown))))?;
        db.set_served_connections(Box::new(Operator::of(&shown)))?;
        Ok(Server { db, authenticators: Vec::new(), shown })
    }

    /// The database served.
    pub fn database(&self) -> &Database {
        &self.db
    }

    /// What ends its connections at an operator's word, kept apart from
    /// the server, so it is used while the server serves.
    pub fn operator(&self) -> Operator {
        Operator::of(&self.shown)
    }

    /// Adds `authenticator`, for clients that ask for its mechanism.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one of its mechanism is added already.
    pub fn add_authenticator(&mut self, authenticator: Arc<dyn Authenticator>) -> Result<()> {
        if self.authenticators.iter().any(|a| a.mechanism() == authenticator.mechanism()) {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] mechanism {} has an authenticator", authenticator.mechanism())));
        }
        self.authenticators.push(authenticator);
        Ok(())
    }

    /// Serves `endpoints` until `stop` is done, as `serve_until` does, with
    /// no second telling to hurry it.
    pub async fn serve(self, endpoints: Vec<Endpoint>, stop: impl Future<Output = ()>) {
        self.serve_until(endpoints, stop, std::future::pending()).await;
    }

    /// Serves `endpoints` until `stop` is done, first polled once it takes
    /// connections and has told `listening`; then takes no connection,
    /// lets each finish its statement and closes it, and returns once all
    /// are. Past SERVER.SHUTDOWN_TIMEOUT_MS, or once `hurry` is done, which is
    /// first polled once `stop` is, so a signal it waits for is the next
    /// one, it hurries: cancels every session's statement and ends every
    /// wait on a client. It runs on the caller's runtime, whose blocking
    /// threads should be as many as SERVER.WORKERS may say
    /// (`settings::MAX_WORKERS`).
    pub async fn serve_until(self, endpoints: Vec<Endpoint>, stop: impl Future<Output = ()>, hurry: impl Future<Output = ()>) {
        let (announce, heard) = watch::channel(Stage::Serving);
        let limits = (Arc::new(Limit::default()), Arc::new(Limit::default()));
        let (cancels, throttles) = (Mutex::new(HashMap::new()), (Throttle::default(), Throttle::default()));
        let (db, authenticators, shown, logins) = (self.db, self.authenticators, self.shown, Logins::default());
        let services =
            Services(Arc::new(Shared { db, authenticators, workers: Arc::default(), stop: heard, cancels, shown, throttles, logins }));
        let addresses = endpoints.iter().map(|e| e.listener.address()).collect();
        let mut listening = JoinSet::new();
        for endpoint in endpoints {
            listening.spawn(listen(endpoint, services.clone(), (Arc::clone(&limits.0), Arc::clone(&limits.1))));
        }
        services.database().tell(&Event::Listening { addresses });
        stop.await;
        // Nothing can fail to hear it: every receiver is in `services`.
        let _ = announce.send(Stage::Stopping);
        services.database().tell(&Event::Stopping { connections: u64::try_from(limits.0.held()).unwrap_or(u64::MAX) });
        let deadline = async {
            match millis(services.database(), SHUTDOWN_TIMEOUT_MS) {
                Some(limit) => tokio::time::sleep(limit).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = drained(&mut listening) => return,
            () = deadline => {}
            () = hurry => {}
        }
        let _ = announce.send(Stage::Hurried);
        services.database().tell(&Event::StopHurried { sessions: services.cancel_all() });
        loop {
            tokio::select! {
                () = drained(&mut listening) => return,
                () = tokio::time::sleep(RECANCEL) => { services.cancel_all(); }
            }
        }
    }
}

/// Done once every task of `tasks` is.
async fn drained(tasks: &mut JoinSet<()>) {
    while tasks.join_next().await.is_some() {}
}

/// Takes `endpoint`'s connections, each served, or refused, on a task of its
/// own, while it has a place among `limits`, the connections and the
/// refusals in flight, else closed at once; until the server stops; then
/// waits for every one to end.
async fn listen(endpoint: Endpoint, services: Services, (connections, refusals): (Arc<Limit>, Arc<Limit>)) {
    let mut open = JoinSet::new();
    loop {
        tokio::select! {
            () = services.stopping() => break,
            Some(_) = open.join_next(), if !open.is_empty() => {}
            accepted = endpoint.listener.accept() => match accepted {
                Ok((transport, peer)) => {
                    let limits = (count(services.database(), MAX_CONNECTIONS), count(services.database(), MAX_REFUSALS));
                    if let Some(place) = place(&connections, &refusals, limits) {
                        let arrival = Arrival { protocol: Arc::clone(&endpoint.protocol), tls: endpoint.tls.clone(), services: services.clone() };
                        open.spawn(arrival.welcome(transport, peer, place));
                    } else {
                        services.0.shown.closed.fetch_add(1, Ordering::Relaxed);
                        services.refused();
                        drop(transport);
                    }
                }
                Err(e) => {
                    services.accept_failed(&e);
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
        }
    }
    drop(endpoint.listener);
    while open.join_next().await.is_some() {}
}

/// What a connection taken is served with.
struct Arrival {
    protocol: Arc<dyn Protocol>,
    tls: Option<Arc<Tls>>,
    services: Services,
}

impl Arrival {
    /// Secures `transport` as the endpoint says, then serves it if its
    /// `place` is under the limit of connections, else refuses it; in its
    /// own task, so neither TLS nor a refusal holds up the connections after
    /// it. Its place, and its row of CONNECTIONS, are given back as its
    /// stream shuts down or is dropped, before its client can see it
    /// closed (`holding.rs`): the stream is held so below TLS, for a
    /// handshake that fails, and above, before TLS's close_notify.
    async fn welcome(self, transport: Box<dyn Transport>, peer: Peer, place: Place) {
        let arrived = Instant::now();
        let (served, taken) = match place {
            Place::Served(taken) => (true, taken),
            Place::Refused(taken) => (false, taken),
        };
        let kept = Hold::new();
        kept.keep(taken);
        let transport: Box<dyn Transport> = Box::new(Holding::new(transport, kept.clone()));
        let Some((secured, peer)) = self.secured(transport, peer, arrived).await else { return };
        let above: Box<dyn Transport> = Box::new(Holding::new(secured, kept.clone()));
        if served {
            let (present, listed) = self.services.0.shown.connected(&peer);
            kept.keep(listed);
            self.protocol.serve(Connection { transport: above, peer, services: self.services, present, arrived }).await;
        } else {
            self.services.0.shown.refused.fetch_add(1, Ordering::Relaxed);
            self.services.refused();
            self.protocol.refuse(above, too_many()).await;
        }
    }

    /// `transport` and `peer` secured, if the endpoint has TLS and the
    /// client begins it; none if the handshake fails, or the client is in
    /// the clear where TLS is required, which it is told.
    async fn secured(&self, transport: Box<dyn Transport>, peer: Peer, arrived: Instant) -> Option<(Box<dyn Transport>, Peer)> {
        let Some(tls) = &self.tls else { return Some((transport, peer)) };
        let handshake = self.services.login(arrived, async { tls.secure(transport).await.map_err(|e| broken(e.to_string())) });
        let name = peer.name();
        let told = |e: &SqlError| self.services.database().tell(&Event::TlsFailed { peer: name, message: e.msg.clone() });
        match handshake.await {
            Ok(Secured::Tls(secured, Handshaken { certificate, end_point, exporter })) => {
                Some((secured, Peer { secure: true, certificate, end_point, exporter, ..peer }))
            }
            Ok(Secured::Plain(plain)) => {
                if tls.required() {
                    self.protocol.refuse(plain, clear()).await;
                    return None;
                }
                Some((plain, peer))
            }
            // A client whose handshake failed cannot be told why in the
            // protocol: TLS has told it. The operator is.
            Err(e) => {
                told(&e);
                None
            }
        }
    }
}

/// The refusal of a client in the clear where TLS is required (SQLSTATE
/// 08004).
fn clear() -> SqlError {
    SqlError::new(Code::Connection, "[DESIGN] this server takes TCP connections over TLS only").with_state(SqlState::CONNECTION_REJECTED)
}

/// The refusal of a connection past the limit (SQLSTATE 08004).
fn too_many() -> SqlError {
    SqlError::new(Code::Connection, "[DESIGN] the server has as many connections as it takes").with_state(SqlState::CONNECTION_REJECTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D42, D55: a connection taken is served
    /// while fewer than the most are; past them, refused while fewer than
    /// SERVER.MAX_REFUSALS refusals are in flight; past these, given no
    /// place, to be closed at once. A place given back makes room again.
    #[test]
    fn connections_past_the_limit_are_refused_then_closed() {
        let (connections, refusals) = (Arc::new(Limit::default()), Arc::new(Limit::default()));
        let limits = (1, crate::settings::DEFAULT_MAX_REFUSALS);
        let served = place(&connections, &refusals, limits);
        assert!(matches!(served, Some(Place::Served(_))));
        let refused: Vec<Option<Place>> = (0..limits.1).map(|_| place(&connections, &refusals, limits)).collect();
        assert!(refused.iter().all(|p| matches!(p, Some(Place::Refused(_)))));
        assert!(place(&connections, &refusals, limits).is_none(), "closed at once");
        assert!(place(&connections, &refusals, (1, 0)).is_none(), "none refused, told why, past 0");
        drop(refused);
        assert!(matches!(place(&connections, &refusals, limits), Some(Place::Refused(_))), "room again");
        drop(served);
        assert!(matches!(place(&connections, &refusals, limits), Some(Place::Served(_))));
    }
}
