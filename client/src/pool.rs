//! \[DESIGN\] A pool of connections (RULES.md D206): a bounded set a
//! program's threads share. `Pool::get` hands out an idle connection that
//! answers, or makes one while fewer than `PoolConfig::max` are made, or
//! waits for one to come back, at most `PoolConfig::wait`. A connection
//! handed out comes back as its `Pooled` is dropped, reset to the state
//! it logged in with (`reset.rs`); one lost, or whose reset fails, is
//! closed instead, so no borrower sees another's session. Idle beyond
//! `PoolConfig::idle_timeout`, or older than `PoolConfig::max_lifetime`,
//! a connection is closed rather than handed out, and no more than
//! `PoolConfig::max_idle` are kept idle. It is thread-safe through the
//! standard library's mutex and condition variable alone, and starts no
//! thread, each wait being the waiting borrower's.

mod reset;

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use turborel::error::{Code, SqlError, SqlState};
use turborel_protocol::Failure;

use crate::agent::Connected;
use crate::connection::Result;
use crate::settings::Settings;

/// \[DESIGN\] The connections a pool makes at most by default.
pub const DEFAULT_MAX: usize = 10;
/// \[DESIGN\] How long `get` waits by default for a connection to come
/// back: as `timeouts::CONNECT_WITHIN`, as long as a person waits.
pub const DEFAULT_WAIT: Duration = crate::timeouts::CONNECT_WITHIN;
/// \[DESIGN\] How long a connection may stay idle by default: ten minutes.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// \[DESIGN\] How long a connection lives by default: an hour, so a pool
/// moves to servers added and settings changed in time.
pub const DEFAULT_MAX_LIFETIME: Duration = Duration::from_secs(3600);

/// What makes a pool's connections.
pub trait Source: Send + Sync {
    /// A new connection, logged in.
    ///
    /// # Errors
    ///
    /// Why none can be made.
    fn connect(&self) -> Result<Connected>;
}

impl Source for Settings {
    fn connect(&self) -> Result<Connected> {
        Settings::connect(self)
    }
}

impl<F: Fn() -> Result<Connected> + Send + Sync> Source for F {
    fn connect(&self) -> Result<Connected> {
        self()
    }
}

/// How a pool keeps its connections.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// The most connections made at once, handed out and idle.
    pub max: usize,
    /// The most kept idle.
    pub max_idle: usize,
    /// How long one may stay idle; none, as long as it lives.
    pub idle_timeout: Option<Duration>,
    /// How long one lives from when it was made; none, without limit.
    pub max_lifetime: Option<Duration>,
    /// How long `get` waits for one to come back.
    pub wait: Duration,
    /// Whether an idle connection is checked to answer before it is
    /// handed out.
    pub check: bool,
    /// Statements run, after the pool's own, as each connection comes
    /// back: what else a program's sessions change and the next must
    /// not see.
    pub reset: Vec<String>,
}

impl Default for PoolConfig {
    fn default() -> PoolConfig {
        PoolConfig {
            max: DEFAULT_MAX,
            max_idle: DEFAULT_MAX,
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
            max_lifetime: Some(DEFAULT_MAX_LIFETIME),
            wait: DEFAULT_WAIT,
            check: true,
            reset: Vec::new(),
        }
    }
}

/// A connection idle in the pool: when it was made, and since when idle.
struct Idle {
    connection: Connected,
    made: Instant,
    since: Instant,
}

/// The pool's connections: those idle, and how many are made.
struct State {
    idle: Vec<Idle>,
    made: usize,
}

/// What a pool's clones share.
struct Shared<S: Source> {
    source: S,
    config: PoolConfig,
    state: Mutex<State>,
    returned: Condvar,
}

/// A pool of connections, shared by its clones.
pub struct Pool<S: Source> {
    shared: Arc<Shared<S>>,
}

impl<S: Source> Clone for Pool<S> {
    fn clone(&self) -> Self {
        Pool { shared: Arc::clone(&self.shared) }
    }
}

/// A connection handed out by a pool, given back as it is dropped.
pub struct Pooled<S: Source> {
    connection: Connected,
    made: Instant,
    shared: Arc<Shared<S>>,
}

/// \[DESIGN\] The stream of the connection that stands in a `Pooled` for
/// the one it gives back, which reads nothing and writes nothing.
struct GivenBack;

impl std::io::Read for GivenBack {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}

impl std::io::Write for GivenBack {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from(std::io::ErrorKind::NotConnected))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl crate::timeouts::Bound for GivenBack {
    fn bound(&self, _: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
    fn gone(&self) -> bool {
        true
    }
}

/// A failure of the pool's own: its lock poisoned (SQLCODE -910,
/// R-ERR-4), or no connection in time.
fn pool_failed(code: Code, msg: &str) -> Failure {
    let e = SqlError::new(code, msg);
    let e = if code == Code::Connection { e.with_state(SqlState::UNABLE_TO_CONNECT) } else { e };
    Failure::from(&e)
}

impl<S: Source> Shared<S> {
    /// The state, or a failure if a thread panicked holding it.
    fn state(&self) -> Result<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| pool_failed(Code::Api, "[DESIGN] the pool's lock is poisoned: a thread panicked holding it"))
    }

    /// Whether `idle` may still be handed out at `now`.
    fn fresh(&self, idle: &Idle, now: Instant) -> bool {
        let lived = self.config.max_lifetime.is_none_or(|most| now.saturating_duration_since(idle.made) < most);
        let rested = self.config.idle_timeout.is_none_or(|most| now.saturating_duration_since(idle.since) < most);
        lived && rested && !idle.connection.is_lost()
    }

    /// One fewer connection made, so a waiter may make one.
    fn unmade(&self) {
        if let Ok(mut state) = self.state() {
            state.made = state.made.saturating_sub(1);
        }
        self.returned.notify_one();
    }
}

/// What `get` takes next: an idle connection, a place to make one, or
/// nothing yet.
enum Next {
    Idle(Box<Idle>),
    Make,
    Wait,
}

impl<S: Source> Pool<S> {
    /// A pool of connections `source` makes, kept as `config` says.
    pub fn new(source: S, config: PoolConfig) -> Pool<S> {
        let state = Mutex::new(State { idle: Vec::new(), made: 0 });
        Pool { shared: Arc::new(Shared { source, config, state, returned: Condvar::new() }) }
    }

    /// A connection: an idle one that answers, else a new one while the
    /// pool has room, else the first to come back within
    /// `PoolConfig::wait`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08001 if none comes back in time; the source's failure to
    /// make one; SQLCODE -910 if the pool's lock is poisoned.
    pub fn get(&self) -> Result<Pooled<S>> {
        let deadline = Instant::now().checked_add(self.shared.config.wait);
        loop {
            match self.next(deadline)? {
                Next::Idle(idle) => {
                    if let Some(pooled) = self.checked(*idle) {
                        return Ok(pooled);
                    }
                }
                Next::Make => return self.make(),
                Next::Wait => {
                    let msg = format!(
                        "[DESIGN] no connection of the pool's {} came back within {:?}",
                        self.shared.config.max, self.shared.config.wait
                    );
                    return Err(pool_failed(Code::Connection, &msg));
                }
            }
        }
    }

    /// What `get` takes next, waiting for a connection to come back until
    /// `deadline`; connections gone stale closed on the way.
    fn next(&self, deadline: Option<Instant>) -> Result<Next> {
        let mut state = self.shared.state()?;
        loop {
            let now = Instant::now();
            while let Some(idle) = state.idle.pop() {
                if self.shared.fresh(&idle, now) {
                    return Ok(Next::Idle(Box::new(idle)));
                }
                state.made = state.made.saturating_sub(1);
            }
            if state.made < self.shared.config.max {
                state.made = state.made.saturating_add(1);
                return Ok(Next::Make);
            }
            let left = deadline.map_or(Duration::ZERO, |d| d.saturating_duration_since(now));
            if left.is_zero() {
                return Ok(Next::Wait);
            }
            let waited = self.shared.returned.wait_timeout(state, left);
            state = waited.map_err(|_| pool_failed(Code::Api, "[DESIGN] the pool's lock is poisoned: a thread panicked holding it"))?.0;
        }
    }

    /// `idle`, handed out if it answers or need not be checked; else
    /// closed, its place given up.
    fn checked(&self, idle: Idle) -> Option<Pooled<S>> {
        let Idle { mut connection, made, .. } = idle;
        if self.shared.config.check && reset::answers(&mut connection).is_err() {
            self.shared.unmade();
            return None;
        }
        Some(Pooled { connection, made, shared: Arc::clone(&self.shared) })
    }

    /// A new connection, in the place `next` took for it.
    fn make(&self) -> Result<Pooled<S>> {
        match self.shared.source.connect() {
            Ok(connection) => Ok(Pooled { connection, made: Instant::now(), shared: Arc::clone(&self.shared) }),
            Err(failure) => {
                self.shared.unmade();
                Err(failure)
            }
        }
    }

    /// How many connections are made, handed out and idle, and how many
    /// of them are idle.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the pool's lock is poisoned.
    pub fn counts(&self) -> Result<(usize, usize)> {
        let state = self.shared.state()?;
        Ok((state.made, state.idle.len()))
    }
}

impl<S: Source> std::ops::Deref for Pooled<S> {
    type Target = Connected;
    fn deref(&self) -> &Connected {
        &self.connection
    }
}

impl<S: Source> std::ops::DerefMut for Pooled<S> {
    fn deref_mut(&mut self) -> &mut Connected {
        &mut self.connection
    }
}

impl<S: Source> Drop for Pooled<S> {
    /// Gives the connection back, reset; or closes it, if it is lost, its
    /// reset fails, it has lived its time, or enough are idle.
    fn drop(&mut self) {
        let mut connection = std::mem::replace(&mut self.connection, Connected::stand_in(Box::new(GivenBack)));
        let shared = &self.shared;
        let lived = shared.config.max_lifetime.is_some_and(|most| self.made.elapsed() >= most);
        let kept = !lived && reset::reset(&mut connection, &shared.config.reset).is_ok();
        let Ok(mut state) = shared.state() else { return };
        if kept && state.idle.len() < shared.config.max_idle {
            state.idle.push(Idle { connection, made: self.made, since: Instant::now() });
        } else {
            state.made = state.made.saturating_sub(1);
        }
        drop(state);
        shared.returned.notify_one();
    }
}
