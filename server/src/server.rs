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

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinSet;
use turborel::db::Database;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::txn::Stop;
use turborel_protocol::{CancelKey, KEY_BYTES, broken};

use crate::auth::{Authenticator, Peer};
use crate::limit::{Held, Limit};
use crate::password::Pbkdf2;
use crate::settings::{IDLE_TIMEOUT, LOGIN_TIMEOUT, MAX_CONNECTIONS, WORKERS, count, declare, millis};
use crate::tls::{Secured, Tls};
use crate::transport::{Listener, Transport};

/// \[DESIGN\] How long a listener waits after failing to accept, so a
/// failure that lasts, such as running out of file descriptors, does not
/// keep a thread busy.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// \[DESIGN\] The connections past SERVER.MAX_CONNECTIONS refused at once,
/// each told why (SQLSTATE 08004) once its TLS, if any, is begun: a
/// refusal over TLS lasts as long as its handshake. One past these is
/// closed at once, telling it nothing, so clients that flood the server
/// hold no more of it than this (RULES.md D55).
const REFUSALS_IN_FLIGHT: usize = 16;

/// A connection's place: under SERVER.MAX_CONNECTIONS, served; or among
/// the refusals in flight, refused.
enum Place {
    Served(Held),
    Refused(Held),
}

/// The place of a connection taken: under `most` of `connections`, else
/// among the refusals in flight; none past these.
fn place(connections: &Arc<Limit>, refusals: &Arc<Limit>, most: usize) -> Option<Place> {
    connections.try_take(most).map(Place::Served).or_else(|| refusals.try_take(REFUSALS_IN_FLIGHT).map(Place::Refused))
}

/// What a `Protocol` returns.
pub type Serving = Pin<Box<dyn Future<Output = ()> + Send>>;

/// \[DESIGN\] A wire protocol: an extension point (RULES.md D39, D42), so
/// more than one can serve the same database.
///
/// Contract (RULES.md R-SAFE-3): `serve` runs a connection to its end:
/// authenticates it through the services' authenticators, runs each
/// statement on a worker (`Services::run`), and ends when the client goes
/// or the server stops (`Services::stopping`), its session ended on a
/// worker. `refuse` tells the client why it is refused, and closes.
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
    stop: watch::Receiver<bool>,
    /// What cancels each connection's statements, by its key (RULES.md D45).
    cancels: Mutex<HashMap<CancelKey, Arc<Stop>>>,
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

    /// The authenticator of `mechanism`, if the server has one.
    pub fn authenticator(&self, mechanism: &str) -> Option<Arc<dyn Authenticator>> {
        self.0.authenticators.iter().find(|a| a.mechanism() == mechanism).cloned()
    }

    /// Runs `work` on a worker, once one is free: once fewer statements
    /// run than SERVER.WORKERS says.
    ///
    /// # Errors
    ///
    /// A connection failure if the worker failed, which is a defect.
    pub async fn run<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
        let permit = self.0.workers.take(|| count(&self.0.db, WORKERS)).await;
        let ran = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        });
        ran.await.map_err(|_| broken("[DESIGN] a worker failed"))
    }

    /// `work`, if it is done within SERVER.LOGIN_TIMEOUT: as a connection
    /// is secured and authenticated, so a client that never finishes does
    /// not hold its place.
    ///
    /// # Errors
    ///
    /// `work`'s; a connection failure if the time ran out.
    pub async fn login<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        bounded(self.database(), LOGIN_TIMEOUT, work, "to begin TLS and authenticate").await
    }

    /// `work`, if it is done within SERVER.IDLE_TIMEOUT: as the server waits
    /// on its client, for a request or to take what it is sent.
    ///
    /// # Errors
    ///
    /// `work`'s; a connection failure if the time ran out.
    pub async fn idle<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        bounded(self.database(), IDLE_TIMEOUT, work, "with nothing from or to the client").await
    }

    /// Done once the server stops.
    pub async fn stopping(&self) {
        let mut stop = self.0.stop.clone();
        // An error means the server is gone: stopped, too.
        let _ = stop.wait_for(|stopped| *stopped).await;
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

/// `work`, if it is done within the milliseconds of the setting `name`;
/// none of 0, no limit. The connection fails once the time runs out,
/// having waited `waiting`.
async fn bounded<T>(db: &Database, name: &str, work: impl Future<Output = Result<T>>, waiting: &str) -> Result<T> {
    let Some(limit) = millis(db, name) else { return work.await };
    tokio::time::timeout(limit, work)
        .await
        .unwrap_or_else(|_| Err(broken(format!("[DESIGN] the connection went past {name}, {} ms, {waiting}: closed", limit.as_millis()))))
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
}

impl Server {
    /// A server of `db`, with no authenticator yet; the server's settings
    /// declared on `db`, unless they are already, and its passwords'
    /// verifiers made as they say (`password::Pbkdf2`), for the users SQL
    /// creates.
    ///
    /// # Errors
    ///
    /// `settings::declare`'s.
    pub fn new(db: Database) -> Result<Server> {
        declare(&db)?;
        db.set_credentials(Box::new(Pbkdf2::of(&db)));
        Ok(Server { db, authenticators: Vec::new() })
    }

    /// The database served.
    pub fn database(&self) -> &Database {
        &self.db
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

    /// Serves `endpoints` until `stop` is done; then takes no connection,
    /// lets each finish its statement and closes it, and returns once all
    /// are. It runs on the caller's runtime, whose blocking threads should
    /// be as many as SERVER.WORKERS may say (`settings::MAX_WORKERS`).
    pub async fn serve(self, endpoints: Vec<Endpoint>, stop: impl Future<Output = ()>) {
        let (announce, heard) = watch::channel(false);
        let connections = Arc::new(Limit::default());
        let refusals = Arc::new(Limit::default());
        let cancels = Mutex::new(HashMap::new());
        let shared = Shared { db: self.db, authenticators: self.authenticators, workers: Arc::default(), stop: heard, cancels };
        let services = Services(Arc::new(shared));
        let mut listening = JoinSet::new();
        for endpoint in endpoints {
            listening.spawn(listen(endpoint, services.clone(), (Arc::clone(&connections), Arc::clone(&refusals))));
        }
        stop.await;
        // Nothing can fail to hear it: every receiver is in `services`.
        let _ = announce.send(true);
        while listening.join_next().await.is_some() {}
    }
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
                    if let Some(place) = place(&connections, &refusals, count(services.database(), MAX_CONNECTIONS)) {
                        let arrival = Arrival { protocol: Arc::clone(&endpoint.protocol), tls: endpoint.tls.clone(), services: services.clone() };
                        open.spawn(arrival.welcome(transport, peer, place));
                    }
                }
                Err(_) => tokio::time::sleep(ACCEPT_BACKOFF).await,
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
    /// it. Its place is given back as it ends.
    async fn welcome(self, transport: Box<dyn Transport>, peer: Peer, place: Place) {
        let Some((transport, peer)) = self.secured(transport, peer).await else { return };
        match place {
            Place::Served(held) => {
                self.protocol.serve(Connection { transport, peer, services: self.services }).await;
                drop(held);
            }
            Place::Refused(held) => {
                self.protocol.refuse(transport, too_many()).await;
                drop(held);
            }
        }
    }

    /// `transport` and `peer` secured, if the endpoint has TLS and the
    /// client begins it; none if the handshake fails, or the client is in
    /// the clear where TLS is required, which it is told.
    async fn secured(&self, transport: Box<dyn Transport>, peer: Peer) -> Option<(Box<dyn Transport>, Peer)> {
        let Some(tls) = &self.tls else { return Some((transport, peer)) };
        let handshake = self.services.login(async { tls.secure(transport).await.map_err(|e| broken(e.to_string())) });
        match handshake.await {
            Ok(Secured::Tls(secured)) => Some((secured, Peer { secure: true, ..peer })),
            Ok(Secured::Plain(plain)) => {
                if tls.required() {
                    self.protocol.refuse(plain, clear()).await;
                    return None;
                }
                Some((plain, peer))
            }
            // A client whose handshake failed cannot be told why in the
            // protocol: TLS has told it.
            Err(_) => None,
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
    /// `REFUSALS_IN_FLIGHT` refusals are in flight; past these, given no
    /// place, to be closed at once. A place given back makes room again.
    #[test]
    fn connections_past_the_limit_are_refused_then_closed() {
        let (connections, refusals) = (Arc::new(Limit::default()), Arc::new(Limit::default()));
        let served = place(&connections, &refusals, 1);
        assert!(matches!(served, Some(Place::Served(_))));
        let refused: Vec<Option<Place>> = (0..REFUSALS_IN_FLIGHT).map(|_| place(&connections, &refusals, 1)).collect();
        assert!(refused.iter().all(|p| matches!(p, Some(Place::Refused(_)))));
        assert!(place(&connections, &refusals, 1).is_none(), "closed at once");
        drop(refused);
        assert!(matches!(place(&connections, &refusals, 1), Some(Place::Refused(_))), "room again");
        drop(served);
        assert!(matches!(place(&connections, &refusals, 1), Some(Place::Served(_))));
    }
}
