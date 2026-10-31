//! \[DESIGN\] `turboreld`, the turborel server's program (RULES.md D42): its
//! command line, and a server run from it until it is told to stop, then
//! the database closed. A library, `main.rs` its one caller, so tests
//! reach all of it; the server itself is `turborel_server`'s, for any
//! program to run.
//!
//! Where the server listens and its limits are the database's settings
//! (RULES.md D36), as all its configuration is: `--set NAME=VALUE` sets
//! one as the program starts, kept with the database as any setting is;
//! `--config FILE` sets those a file gives, in a subset of TOML, before
//! (`config_file.rs`).
//!
//! Signals (Unix): SIGTERM, as Ctrl-C everywhere, stops the server, and a
//! second hurries it (`Server::serve_until`); SIGHUP opens the `--log`
//! file again, for a rotation that has moved it away, and takes TLS's
//! certificate, key and client authorities again (`Tls::reload`), for a
//! certificate renewed in place. Windows has no SIGHUP: a log there is
//! rotated with the server stopped, and TLS's files are taken again as
//! their settings change, as everywhere.
//!
//! Clients prove who they are by `password`, `scram-sha-256` or
//! `certificate`, or, with SERVER.TRUST_LOCAL ON, are trusted on this machine.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;

use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::files::Noted;
use turborel::settings::Effect;
use turborel::value::{Decimal, Value};
use turborel_aes::{AesGcm, ChaCha20Poly1305, HmacSha256, KEY_BYTES, KeyDirectory, Keyring, Sha256, Sha512};
use turborel_lz4::Lz4;

use tokio::runtime::Runtime;
use turborel_server::auth::{Authenticator, Trust};
use turborel_server::certificate::Certificate;
use turborel_server::listening;
use turborel_server::log::{Lines, Reopener};
use turborel_server::native::Native;
use turborel_server::password::{Password, Pbkdf2};
use turborel_server::scram::Scram;
use turborel_server::server::{Protocol, Server};
use turborel_server::settings::{self, TRUST_LOCAL};
use turborel_server::tls::Tls;

/// The usage text.
pub const USAGE: &str = "usage: turboreld (--dir PATH | --memory) [options]
       turboreld --dir PATH [--config FILE] [--set NAME=VALUE]... user COMMAND
       turboreld --dir PATH [--keys DIR] reseal AUTH.TABLE

  --dir PATH             serve the database kept in PATH, created if there is none
  --memory               serve a database kept nowhere, lost when the server stops
  --owner NAME           with --memory: make NAME its first user and owner as the
                         server starts, the password typed or read from standard input
  --set NAME=VALUE       set the database's setting NAME as the server starts, kept
                         with the database; more than once for more
  --config FILE          set the database's settings FILE gives, in a subset of TOML, as
                         --set does; a setting --set gives too is set as --set gives it
  --log FILE             append the diagnostics, a line of JSON each, to FILE
                         (default: standard error)
  --keys DIR             the encryption keys: a file in DIR each, named as the key,
                         of 64 hexadecimal digits (openssl rand -hex 32)
  --checksum-key NAME    the key of --keys that HMAC-SHA-256 checksums are keyed by
  --help                 print this
  --version              print the release

User commands, on a database no server holds; a password is typed, or read from
standard input:
  user add NAME [--owner]   a user, the database's owner if asked
  user password NAME        a user's new password
  user drop NAME            drop a user
  user owner NAME           make a user the database's owner
  user list                 every user, the owner marked

A table's rows sealed again, through the encodings it names now, on a database no
server holds: then a key it named before may be retired:
  reseal AUTH.TABLE

The server's settings, beside the database's:
  SERVER.HOST            hosts or addresses, by commas, to take TCP connections at
                         (default 127.0.0.1; empty for none)
  SERVER.PORT            their port (default 7422; 0 for one the system chooses)
  SERVER.SOCKET          a Unix socket, on Windows a named pipe, to take connections at
                         (default none)
  SERVER.TLS             OFF, ON (TLS offered on the TCP port) or REQUIRED (default OFF)
  SERVER.TLS_CERTIFICATE the PEM file of the server's certificate chain
  SERVER.TLS_KEY         the PEM file of the server's private key
  SERVER.TLS_CLIENT_CA   the PEM file of the authorities whose certificates clients
                         may present to log in by (default none)
  SERVER.SCRAM_WITHOUT_TLS  ON: take SCRAM-SHA-256 over plain TCP too (default OFF)
  SERVER.WORKERS         statements run at once (default: one per processor)
  SERVER.LOGIN_TIMEOUT_MS  milliseconds a client has to begin TLS and authenticate
                         (default 60000; 0 for no limit)
  SERVER.MAX_LOGINS      authentications checked at once, apart from the workers (default 4)
  SERVER.LOGIN_DELAY_MS  the longest a login waits after failed ones from its address
                         (default 10000; 0 for no wait)
  SERVER.IDLE_TIMEOUT_MS milliseconds the server waits on a client, for a request or to
                         take its rows, before closing it and rolling it back (default 0, none)
  SERVER.SHUTDOWN_TIMEOUT_MS  milliseconds a server told to stop lets statements finish,
                         before it cancels them (default 30000; 0 for no limit)
  SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS  milliseconds the server waits for a request of a
                         client in a transaction, before closing it and rolling it back
                         (default 0, none)
  SERVER.KEEPALIVE_IDLE_MS  milliseconds a TCP connection is idle before the system
                         probes its client (default 0, the system's own)
  SERVER.KEEPALIVE_INTERVAL_MS  milliseconds between probes unanswered (default 0,
                         the system's own)
  SERVER.KEEPALIVE_PROBES  probes unanswered before the connection is closed
                         (default 0, the system's own)
  SERVER.MAX_CONNECTIONS connections taken at once (default 256)
  SERVER.MAX_REFUSALS    connections past those refused at once, told why; past these,
                         closed untold (default 16)
  SERVER.MAX_MESSAGE_BYTES  the longest message taken from a client (default 16 MiB)
  SERVER.MAX_STATEMENTS  prepared statements a connection holds at once (default 1024)
  SERVER.BATCH_BYTES     about how many bytes of rows go in a batch (default 64 KiB)
  SERVER.TRUST_LOCAL     OFF: every client proves who it is (default);
                         ON: trust a client of this machine as who it says
  SERVER.PASSWORD_ITERATIONS  PBKDF2 iterations of a password made (default 600000)

The database's files, fixed when it is created: --set them as it is:
  STORAGE.PAGE_BYTES     the bytes of a page of its data file, a power of two (default 8192)
  STORAGE.CHECKSUM       the hash function its files are checksummed with (default CRC-32/ISCSI;
                         SHA-256, or HMAC-SHA-256 keyed by --checksum-key)
  STORAGE.ENCODINGS      the encodings its files pass through, by commas: LZ4, and
                         AES-256-GCM:KEY or ChaCha20-Poly1305:KEY with a key of --keys
                         (default none)
  LOG.SEGMENT_BYTES      the bytes of a segment of its log (default 64 MiB)
  STORAGE.POOL_BYTES     the bytes of its buffer pool, as it opens (default 32 MiB)

Clients prove who they are by a user's password, sent or proved by SCRAM-SHA-256, over
TLS or a Unix socket only, or by a certificate of SERVER.TLS_CLIENT_CA's authorities
whose common name is the user's; with SERVER.TRUST_LOCAL ON, a client of this machine is
trusted instead. The database's
owner manages users in SQL (CREATE USER, ALTER USER, DROP USER), and sets the database's
settings, the server's among them, on a running server: SET DATABASE NAME = VALUE.
Ctrl-C, or SIGTERM on Unix, stops the server: each connection finishes its statement,
then the database is closed. Past SERVER.SHUTDOWN_TIMEOUT_MS milliseconds (default 30000;
0 for no limit), or at a second Ctrl-C or SIGTERM, running statements are cancelled
and connections closed, rolled back. SIGHUP (Unix) opens the --log file again, as
a log rotation asks, and takes TLS's files again, for the connections after.
A table's rows may pass through encodings of their own, compressed or encrypted:
CREATE TABLE ... WITH (ENCODINGS = 'LZ4,AES-256-GCM:KEY'), or ALTER TABLE ... SET (...);
rows written before keep theirs until the table is resealed.
An encrypted table is indexed only where STORAGE.ENCODINGS encrypts the files too.";

/// Where the database is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Store {
    /// In a directory.
    Dir(PathBuf),
    /// Nowhere.
    Memory,
}

/// What the command line asks for.
#[derive(Debug, Clone)]
pub enum Command {
    /// The usage text.
    Help,
    /// The release, as `turborel::product` gives it.
    Version,
    /// A server.
    Serve(Config),
    /// A user command, on the database's directory (`users.rs`).
    User(Config, UserCommand),
    /// A table's rows sealed again, on the database's directory
    /// (`reseal.rs`): the table, as written.
    Reseal(Config, String),
}

/// A server's configuration, from the command line.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where the database is kept.
    pub store: Store,
    /// Settings to set as the server starts: names and values.
    pub settings: Vec<(String, Value)>,
    /// The configuration file of more settings to set, if any
    /// (`config_file.rs`).
    pub file: Option<PathBuf>,
    /// The file the diagnostics are appended to; none for standard error.
    pub log: Option<PathBuf>,
    /// The directory of the encryption keys, if any.
    pub keys: Option<PathBuf>,
    /// The owner made of a database kept nowhere as it is served, if any
    /// (`users.rs`).
    pub owner: Option<Owner>,
    /// The key of `keys` HMAC-SHA-256 is keyed by, if any.
    pub checksum_key: Option<String>,
}

/// What the command line `args`, without the program's name, asks for.
///
/// # Errors
///
/// What is wrong with it, for the operator.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let (mut store, mut settings, mut user) = (None, Vec::new(), None);
    let (mut log, mut keys, mut owner, mut checksum_key, mut reseal) = (None, None, None, None, None);
    let mut file: Option<PathBuf> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--help" => return Ok(Command::Help),
            "--version" => return Ok(Command::Version),
            "--dir" => store = once(store.is_some(), Store::Dir(PathBuf::from(value()?)))?,
            "--memory" => store = once(store.is_some(), Store::Memory)?,
            "--set" => settings.push(setting(value()?)?),
            "--config" => file = one_file(file.is_some(), value()?)?,
            "--log" => log = Some(PathBuf::from(value()?)),
            "--keys" => keys = Some(PathBuf::from(value()?)),
            "--owner" => owner = Some(Owner { name: value()?.clone(), password: None }),
            "--checksum-key" => checksum_key = Some(value()?.clone()),
            "user" => {
                user = Some(parse_user(rest.as_slice())?);
                break;
            }
            "reseal" => {
                reseal = Some(parse_reseal(rest.as_slice())?);
                break;
            }
            other => return Err(format!("{other} is not an option\n\n{USAGE}")),
        }
    }
    let store = store.ok_or_else(|| format!("name --dir or --memory\n\n{USAGE}"))?;
    if owner.is_some() && (store != Store::Memory || user.is_some() || reseal.is_some()) {
        return Err(
            "--owner makes the owner of a database kept nowhere, served with --memory: in a directory, user add NAME --owner".into()
        );
    }
    let config = Config { store, settings, file, log, keys, owner, checksum_key };
    Ok(match (user, reseal) {
        (Some(command), _) => Command::User(config, command),
        (None, Some(table)) => Command::Reseal(config, table),
        (None, None) => Command::Serve(config),
    })
}

/// `store`, unless one was `named` already.
fn once(named: bool, store: Store) -> Result<Option<Store>, String> {
    if named {
        return Err("name one of --dir and --memory, once".into());
    }
    Ok(Some(store))
}

/// The configuration file at `path`, unless one was `named` already.
fn one_file(named: bool, path: &str) -> Result<Option<PathBuf>, String> {
    if named {
        return Err("name one --config file, once".into());
    }
    Ok(Some(PathBuf::from(path)))
}

/// The setting `NAME=VALUE` names, and its value: a whole number, any
/// other number, or else a word.
fn setting(text: &str) -> Result<(String, Value), String> {
    let (name, given) = text.split_once('=').ok_or_else(|| format!("--set takes NAME=VALUE, not {text}"))?;
    let value = match (given.parse::<i128>(), given.parse::<f64>()) {
        (Ok(n), _) => Value::Exact(Decimal::new(n, 0)),
        (Err(_), Ok(x)) => Value::Approx(x),
        (Err(_), Err(_)) => Value::Char(given.to_owned()),
    };
    Ok((name.trim().to_owned(), value))
}

/// Serves `config` until `stop` is done, hurried once `hurry` is
/// (`Server::serve_until`), then closes the database, if it is kept
/// somewhere. `ready` is given where it listens, once it does, its
/// diagnostics have told `listening` and `stop` has been polled once; it
/// runs on the server's runtime, so it should not wait long. From the
/// start, SIGHUP opens the log file again (Unix): so before `ready`, and a
/// signal sent once it is cannot end the process; SIGTERM sent then,
/// heard by `signalled` from its first poll, stops it cleanly.
///
/// # Errors
///
/// What failed, for the operator: opening or closing the database, or
/// listening.
pub fn run(
    config: &Config,
    ready: impl FnOnce(&[String]),
    (stop, hurry): (impl Future<Output = ()>, impl Future<Output = ()>),
) -> Result<(), String> {
    let runtime = turborel_server::runtime::runtime(None).map_err(|e| format!("cannot start: {e}"))?;
    let retaken = Retaken::default();
    within(config, Some((&runtime, &retaken)), |mut server| {
        let db = server.database().clone();
        let credentials = Pbkdf2::of(&db);
        let mut mechanisms: Vec<Arc<dyn Authenticator>> = vec![
            Arc::new(Password::new(db.clone(), &credentials).map_err(|e| e.to_string())?),
            Arc::new(Scram::new(db.clone(), &credentials).map_err(|e| e.to_string())?),
            Arc::new(Certificate::new(db.clone())),
        ];
        if settings::text(&db, TRUST_LOCAL) == turborel::settings::ON {
            mechanisms.push(Arc::new(Trust));
        }
        mechanisms.into_iter().try_for_each(|m| server.add_authenticator(m)).map_err(|e| e.to_string())?;
        runtime.block_on(serve(server, ready, (stop, hurry), &retaken))
    })
}

/// The server's TLS, once it serves, which SIGHUP takes again.
type Retaken = Arc<Mutex<Option<Arc<Tls>>>>;

/// Does `work` with a server of `config`'s database, its settings set as
/// `config` says; then closes the database, if it is kept somewhere,
/// whether `work` failed or not. On `hearing`'s runtime, if given, SIGHUP
/// opens the log file again from the start, and takes again the TLS its
/// slot holds.
///
/// # Errors
///
/// What failed, for the operator: opening or closing the database, setting
/// a setting, or `work`.
fn within(config: &Config, hearing: Option<(&Runtime, &Retaken)>, work: impl FnOnce(Server) -> Result<(), String>) -> Result<(), String> {
    // Read first, so a file that is wrong touches nothing.
    let entries = config_file::entries(config)?;
    let db = Database::new();
    // Before the database opens, so its opening is told too.
    let sink = match &config.log {
        Some(path) => {
            Lines::appending(path).map_err(|e| format!("cannot open the log {}: {e}", path.display()))?.noting(noted(&db, path, true)?)
        }
        None => Lines::to_stderr().map_err(|e| format!("cannot write the log: {e}"))?,
    };
    if let Some((runtime, retaken)) = hearing {
        hangups(runtime, sink.reopener(), Arc::clone(retaken))?;
    }
    db.set_diagnostics(Box::new(sink));
    register(&db, config)?;
    let given = config_file::given(&db, config, entries)?;
    // Those read as the database is created or opened are set before it is.
    let (early, late): (Vec<_>, Vec<_>) = given.iter().partition(|g| read_as_opened(&db, &g.name));
    let set = |g: &&config_file::Given| g.set(&db);
    // A database kept in memory is created in no directory (RULES.md D36).
    let fixed = early.iter().find(|g| effect_of(&db, &g.name) == Some(Effect::Creation)).filter(|_| config.store == Store::Memory);
    if let Some(g) = fixed {
        return Err(format!(
            "cannot set {}: it is fixed when a database is created in its directory, and --memory keeps it in memory",
            g.name
        ));
    }
    early.iter().try_for_each(set)?;
    if let Store::Dir(dir) = &config.store {
        let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
        db.open_files(&fs, dir, &OpenOptions::default()).map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
    }
    let outcome = Server::new(db.clone()).map_err(|e| e.to_string()).and_then(|server| {
        // The server's own, declared by `Server::new`, are set once it is
        // made; the verifiers it makes of the users SQL creates are made
        // again, so they take SERVER.PASSWORD_ITERATIONS as given here.
        late.iter().try_for_each(set)?;
        db.set_credentials(Box::new(Pbkdf2::of(&db)));
        config.owner.as_ref().map_or(Ok(()), |owner| users::make_owner(&db, owner))?;
        work(server)
    });
    let closed = match &config.store {
        Store::Dir(dir) => db.close().map_err(|e| format!("cannot close {}: {e}", dir.display())),
        Store::Memory => Ok(()),
    };
    outcome.and(closed)
}

/// Registers with `db` the plug-ins the program ships (RULES.md D26, D48):
/// LZ4; AES-256-GCM and ChaCha20-Poly1305 with the keys of `--keys`;
/// SHA-256 and SHA-512; and HMAC-SHA-256 if `--checksum-key` names its key.
fn register(db: &Database, config: &Config) -> Result<(), String> {
    db.register_encoding(Box::new(Lz4)).map_err(refused("LZ4"))?;
    let keyring: Arc<dyn Keyring> = match &config.keys {
        Some(dir) => Arc::new(NotedKeys { keys: KeyDirectory::new(dir), noted: noted(db, dir, false)? }),
        None => Arc::new(NoKeys),
    };
    db.register_encoding(Box::new(AesGcm::new(Arc::clone(&keyring)))).map_err(refused("AES-256-GCM"))?;
    db.register_encoding(Box::new(ChaCha20Poly1305::new(Arc::clone(&keyring)))).map_err(refused("ChaCha20-Poly1305"))?;
    db.register_hash_function(Box::new(Sha256)).map_err(refused("SHA-256"))?;
    db.register_hash_function(Box::new(Sha512)).map_err(refused("SHA-512"))?;
    if let Some(name) = &config.checksum_key {
        let keyed = HmacSha256::new(&*keyring, name).map_err(refused("HMAC-SHA-256"))?;
        db.register_hash_function(Box::new(keyed)).map_err(refused("HMAC-SHA-256"))?;
    }
    Ok(())
}

/// What says a plug-in, `what`, could not be registered.
fn refused(what: &'static str) -> impl Fn(turborel::error::SqlError) -> String {
    move |e| format!("cannot register {what}: {e}")
}

/// The file or directory at `path`, the server's, shown among `db`'s files
/// (RULES.md D56), held open now if `open`.
fn noted(db: &Database, path: &Path, open: bool) -> Result<Noted, String> {
    db.note_file(path, open).map_err(|e| format!("cannot note {}: {e}", path.display()))
}

/// The keyring of `--keys`, its directory shown among the database's files,
/// each key read counted.
struct NotedKeys {
    keys: KeyDirectory,
    noted: Noted,
}

impl Keyring for NotedKeys {
    fn name(&self) -> &'static str {
        self.keys.name()
    }
    fn key(&self, name: &str) -> turborel::error::Result<Option<[u8; KEY_BYTES]>> {
        self.noted.read();
        self.keys.key(name)
    }
}

/// The keyring of a server given no `--keys`: it has none, and says how
/// to give them.
struct NoKeys;

impl Keyring for NoKeys {
    fn name(&self) -> &'static str {
        "none: give turboreld --keys DIR"
    }
    fn key(&self, _: &str) -> turborel::error::Result<Option<[u8; KEY_BYTES]>> {
        Ok(None)
    }
}

/// When a change of the setting `name` of `db` takes effect, if it is one.
fn effect_of(db: &Database, name: &str) -> Option<Effect> {
    let settings = db.settings().unwrap_or_default();
    settings.iter().find(|l| l.definition.name.eq_ignore_ascii_case(name.trim())).map(|l| l.definition.effect)
}

/// Whether the setting `name` of `db` is read as a database is created or
/// opened: so set before it is.
fn read_as_opened(db: &Database, name: &str) -> bool {
    matches!(effect_of(db, name), Some(Effect::Creation | Effect::Start))
}

/// Serves `server` where its settings say, until `stop`, hurried at
/// `hurry`; `ready` told where it listens; its TLS, if any, put in
/// `retaken`, for SIGHUP to take again.
async fn serve(
    server: Server,
    ready: impl FnOnce(&[String]),
    (stop, hurry): (impl Future<Output = ()>, impl Future<Output = ()>),
    retaken: &Retaken,
) -> Result<(), String> {
    let native: Arc<dyn Protocol> = Arc::new(Native);
    let endpoints = listening::bind(server.database(), &native).await.map_err(|e| e.to_string())?;
    *retaken.lock().unwrap_or_else(PoisonError::into_inner) = endpoints.iter().find_map(|e| e.tls.clone());
    let names: Vec<String> = endpoints.iter().map(|e| e.listener.address()).collect();
    // Ready as `stop` is first polled: once the server takes connections
    // and has told `listening`, so that line goes to the log a rotation
    // signalled at once moves away. SIGHUP is heard already (`within`).
    // `stop` is polled once before: it hears what stops the server from
    // its first poll (`signalled`), so a SIGTERM sent as soon as the
    // program is ready stops it cleanly, neither ending the process nor
    // lost.
    let stop = async move {
        let mut stop = std::pin::pin!(stop);
        let stopped = std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_ready())).await;
        ready(&names);
        if !stopped {
            stop.await;
        }
    };
    server.serve_until(endpoints, stop, hurry).await;
    Ok(())
}

/// \[DESIGN\] On Unix, SIGHUP heard on `runtime` from now on, each asking
/// `reopener` to open the log file again: for a rotation, which moves the
/// file away and then signals; and the TLS `retaken` holds to take its
/// files again, on a blocking thread, for a certificate renewed in place;
/// and no longer the end of the process it is by default.
///
/// # Errors
///
/// The signal cannot be heard.
#[cfg(unix)]
fn hangups(runtime: &Runtime, reopener: Reopener, retaken: Retaken) -> Result<(), String> {
    use tokio::signal::unix::{SignalKind, signal};
    let _entered = runtime.enter();
    let mut hangup = signal(SignalKind::hangup()).map_err(|e| format!("cannot hear SIGHUP: {e}"))?;
    runtime.spawn(async move {
        while hangup.recv().await.is_some() {
            reopener.reopen();
            let tls = retaken.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(tls) = tls {
                // Told either way (`Tls::reload`).
                drop(tokio::task::spawn_blocking(move || tls.reload()));
            }
        }
    });
    Ok(())
}

/// Off Unix there is no SIGHUP: a log is rotated with the server stopped,
/// and TLS's files are taken again as their settings change.
#[cfg(not(unix))]
#[expect(clippy::unnecessary_wraps, reason = "fails as on Unix, where the signal may not be heard")]
fn hangups(_: &Runtime, reopener: Reopener, retaken: Retaken) -> Result<(), String> {
    drop((reopener, retaken));
    Ok(())
}

/// Done when the program is told to stop, or, polled once it has been, to
/// hurry: Ctrl-C, or SIGTERM on Unix. If it cannot be told, it is done at
/// once, rather than serve with no way to stop cleanly. Each signal is
/// heard from its first poll on.
pub async fn signalled() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

pub mod config_file;
mod reseal;
mod users;

pub use reseal::{parse_reseal, run_reseal};
pub use users::{Owner, UserCommand, parse_user, read_owner_password, run_user};

// Unit tests: the command line read (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
