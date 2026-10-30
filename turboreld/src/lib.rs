//! \[DESIGN\] `turboreld`, the turborel server's program (RULES.md D42): its
//! command line, and a server run from it until it is told to stop, then
//! the database closed. A library, `main.rs` its one caller, so tests
//! reach all of it; the server itself is `turborel_server`'s, for any
//! program to run.
//!
//! Where the server listens and its limits are the database's settings
//! (RULES.md D36), as all its configuration is: `--set NAME=VALUE` sets
//! one as the program starts, kept with the database as any setting is.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::files::Noted;
use turborel::settings::Effect;
use turborel::value::{Decimal, Value};
use turborel_aes::{AesGcm, KEY_BYTES, KeyDirectory, Keyring};
use turborel_lz4::Lz4;

use turborel_server::auth::Trust;
use turborel_server::listening;
use turborel_server::log::Lines;
use turborel_server::native::Native;
use turborel_server::password::{Password, Pbkdf2};
use turborel_server::server::{Protocol, Server};
use turborel_server::settings::{self, TRUST};

/// The usage text.
pub const USAGE: &str = "usage: turboreld (--dir PATH | --memory) [options]
       turboreld --dir PATH [--set NAME=VALUE]... user COMMAND

  --dir PATH             serve the database kept in PATH, created if there is none
  --memory               serve a database kept nowhere, lost when the server stops
  --set NAME=VALUE       set the database's setting NAME as the server starts, kept
                         with the database; more than once for more
  --log FILE             append the diagnostics, a line of JSON each, to FILE
                         (default: standard error)
  --keys DIR             the encryption keys: a file in DIR each, named as the key,
                         of 64 hexadecimal digits (openssl rand -hex 32)
  --help                 print this
  --version              print the release

User commands, on a database no server holds; a password is typed, or read from
standard input:
  user add NAME [--owner]   a user, the database's owner if asked
  user password NAME        a user's new password
  user drop NAME            drop a user
  user owner NAME           make a user the database's owner
  user list                 every user, the owner marked

The server's settings, beside the database's:
  SERVER.HOST            hosts or addresses, by commas, to take TCP connections at
                         (default 127.0.0.1; empty for none)
  SERVER.PORT            their port (default 7422; 0 for one the system chooses)
  SERVER.SOCKET          a Unix socket to take connections at (Unix only; default none)
  SERVER.TLS             OFF, ON (TLS offered on the TCP port) or REQUIRED (default OFF)
  SERVER.TLS_CERTIFICATE the PEM file of the server's certificate chain
  SERVER.TLS_KEY         the PEM file of the server's private key
  SERVER.WORKERS         statements run at once (default: one per processor)
  SERVER.LOGIN_TIMEOUT   milliseconds a client has to authenticate (default 60000; 0 for no limit)
  SERVER.IDLE_TIMEOUT    milliseconds the server waits on a client, for a request or to
                         take its rows, before closing it and rolling it back (default 0, none)
  SERVER.MAX_CONNECTIONS connections taken at once (default 256)
  SERVER.MAX_MESSAGE_BYTES  the longest message taken from a client (default 16 MiB)
  SERVER.BATCH_BYTES     about how many bytes of rows go in a batch (default 64 KiB)
  SERVER.TRUST           OFF: every client proves who it is (default);
                         LOCAL: trust a client of this machine as who it says
  SERVER.PASSWORD_ITERATIONS  PBKDF2 iterations of a password made (default 600000)

The database's files, fixed when it is created: --set them as it is:
  STORAGE.PAGE_SIZE      the bytes of a page of its data file, a power of two (default 8192)
  STORAGE.CHECKSUM       the hash function its files are checksummed with (default CRC-32/ISCSI)
  STORAGE.ENCODINGS      the encodings its files pass through, by commas: LZ4, and
                         AES-256-GCM:KEY with a key of --keys (default none)
  LOG.SEGMENT_SIZE       the bytes of a segment of its log (default 64 MiB)
  STORAGE.POOL_BYTES     the bytes of its buffer pool, as it opens (default 32 MiB)

Clients prove who they are by a user's password, over TLS or a Unix socket only; with
SERVER.TRUST LOCAL, a client of this machine is trusted instead. The database's
owner manages users in SQL (CREATE USER, ALTER USER, DROP USER), and sets the database's
settings, the server's among them, on a running server: SET DATABASE NAME = VALUE.
Ctrl-C, or SIGTERM on Unix, stops the server: each connection finishes its statement,
then the database is closed.
A table's rows may pass through encodings of their own, compressed or encrypted:
CREATE TABLE ... WITH (ENCODINGS = 'LZ4,AES-256-GCM:KEY'), or ALTER TABLE ... SET (...).
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
}

/// A server's configuration, from the command line.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where the database is kept.
    pub store: Store,
    /// Settings to set as the server starts: names and values.
    pub settings: Vec<(String, Value)>,
    /// The file the diagnostics are appended to; none for standard error.
    pub log: Option<PathBuf>,
    /// The directory of the encryption keys, if any.
    pub keys: Option<PathBuf>,
}

/// What the command line `args`, without the program's name, asks for.
///
/// # Errors
///
/// What is wrong with it, for the operator.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let (mut store, mut settings, mut user) = (None, Vec::new(), None);
    let (mut log, mut keys) = (None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--help" => return Ok(Command::Help),
            "--version" => return Ok(Command::Version),
            "--dir" => store = once(store.is_some(), Store::Dir(PathBuf::from(value()?)))?,
            "--memory" => store = once(store.is_some(), Store::Memory)?,
            "--set" => settings.push(setting(value()?)?),
            "--log" => log = Some(PathBuf::from(value()?)),
            "--keys" => keys = Some(PathBuf::from(value()?)),
            "user" => {
                user = Some(parse_user(rest.as_slice())?);
                break;
            }
            other => return Err(format!("{other} is not an option\n\n{USAGE}")),
        }
    }
    let store = store.ok_or_else(|| format!("name --dir or --memory\n\n{USAGE}"))?;
    let config = Config { store, settings, log, keys };
    Ok(match user {
        Some(command) => Command::User(config, command),
        None => Command::Serve(config),
    })
}

/// `store`, unless one was `named` already.
fn once(named: bool, store: Store) -> Result<Option<Store>, String> {
    if named {
        return Err("name one of --dir and --memory, once".into());
    }
    Ok(Some(store))
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

/// Serves `config` until `stop` is done, then closes the database, if it
/// is kept somewhere. `ready` is given where it listens, once it does.
///
/// # Errors
///
/// What failed, for the operator: opening or closing the database, or
/// listening.
pub fn run(config: &Config, ready: impl FnOnce(&[String]), stop: impl Future<Output = ()>) -> Result<(), String> {
    within(config, |mut server| {
        let db = server.database().clone();
        server
            .add_authenticator(Arc::new(Password::new(db.clone(), &Pbkdf2::of(&db)).map_err(|e| e.to_string())?))
            .map_err(|e| e.to_string())?;
        if settings::text(&db, TRUST) == "LOCAL" {
            server.add_authenticator(Arc::new(Trust)).map_err(|e| e.to_string())?;
        }
        let runtime = turborel_server::runtime::runtime(None).map_err(|e| format!("cannot start: {e}"))?;
        runtime.block_on(serve(server, ready, stop))
    })
}

/// Does `work` with a server of `config`'s database, its settings set as
/// `config` says; then closes the database, if it is kept somewhere,
/// whether `work` failed or not.
///
/// # Errors
///
/// What failed, for the operator: opening or closing the database, setting
/// a setting, or `work`.
fn within(config: &Config, work: impl FnOnce(Server) -> Result<(), String>) -> Result<(), String> {
    let db = Database::new();
    // Before the database opens, so its opening is told too.
    let sink = match &config.log {
        Some(path) => {
            Lines::appending(path).map_err(|e| format!("cannot open the log {}: {e}", path.display()))?.noting(noted(&db, path, true)?)
        }
        None => Lines::to_stderr(),
    };
    db.set_diagnostics(Box::new(sink));
    db.register_encoding(Box::new(Lz4)).map_err(|e| format!("cannot register LZ4: {e}"))?;
    let keyring: Arc<dyn Keyring> = match &config.keys {
        Some(dir) => Arc::new(NotedKeys { keys: KeyDirectory::new(dir), noted: noted(&db, dir, false)? }),
        None => Arc::new(NoKeys),
    };
    db.register_encoding(Box::new(AesGcm::new(keyring))).map_err(|e| format!("cannot register AES-256-GCM: {e}"))?;
    // Those read as the database is created or opened are set before it is.
    let (early, late): (Vec<_>, Vec<_>) = config.settings.iter().partition(|(name, _)| read_as_opened(&db, name));
    let set = |(name, value): &&(String, Value)| db.set_setting(name, value).map_err(|e| format!("cannot set {name}: {e}"));
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
        work(server)
    });
    let closed = match &config.store {
        Store::Dir(dir) => db.close().map_err(|e| format!("cannot close {}: {e}", dir.display())),
        Store::Memory => Ok(()),
    };
    outcome.and(closed)
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

/// Whether the setting `name` of `db` is read as a database is created or
/// opened: so set before it is.
fn read_as_opened(db: &Database, name: &str) -> bool {
    let settings = db.settings().unwrap_or_default();
    settings
        .iter()
        .any(|l| l.definition.name.eq_ignore_ascii_case(name.trim()) && matches!(l.definition.effect, Effect::Creation | Effect::Start))
}

async fn serve(server: Server, ready: impl FnOnce(&[String]), stop: impl Future<Output = ()>) -> Result<(), String> {
    let native: Arc<dyn Protocol> = Arc::new(Native);
    let endpoints = listening::bind(server.database(), &native).await.map_err(|e| e.to_string())?;
    let names: Vec<String> = endpoints.iter().map(|e| e.listener.address()).collect();
    ready(&names);
    server.serve(endpoints, stop).await;
    Ok(())
}

/// Done when the program is told to stop: Ctrl-C, or SIGTERM on Unix. If
/// it cannot be told, it is done at once, rather than serve with no way
/// to stop cleanly.
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

mod users;

pub use users::{UserCommand, parse_user, run_user};

// Unit tests: the command line read (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
