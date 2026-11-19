//! \[DESIGN\] Connection settings from outside the program (RULES.md D202,
//! D203): where the servers are, as whom to connect, how, and what a
//! server must be, read from layers of `Options`: the program's own (or a
//! connection URI's), then a service's section of the service file, then
//! the environment, then the defaults, each setting taken from the first
//! layer that has it. A password none of them gives is looked up in the
//! password file. `Settings::connect` connects as they say (`failover.rs`).

mod files;
mod options;
mod uri;
mod values;

use std::path::{Path, PathBuf};
use std::time::Duration;

use turborel::error::{Code, SqlError, SqlState};
use turborel_protocol::Failure;

pub use options::{
    APPLICATION_NAME, CHANNEL_BINDING, CONNECT_TIMEOUT, ENVIRONMENT, HOST, KEYS, LOAD_BALANCE, Options, PASSFILE, PASSWORD, PORT,
    SERVER_NAME, SERVICE, SERVICE_FILE, TARGET, TLS_CERTIFICATE, TLS_KEY, TLS_MODE, TLS_ROOTS, TLS_SYSTEM_ROOTS, USER,
};
pub use uri::SCHEME;
pub use values::{ChannelBinding, LoadBalance, TargetRole, TlsMode};

use crate::connection::Result;
use crate::timeouts::CONNECT_WITHIN;

/// \[DESIGN\] The port of the native protocol (RULES.md D42), where a
/// server listens unless told otherwise.
pub const DEFAULT_PORT: u16 = 7422;
/// \[DESIGN\] The host connected to when none is named.
pub const DEFAULT_HOST: &str = "localhost";
/// \[DESIGN\] The service file read when none is named, in the home
/// directory.
pub const DEFAULT_SERVICE_FILE: &str = ".turborel_service.conf";
/// \[DESIGN\] The password file read when none is named, in the home
/// directory.
pub const DEFAULT_PASSFILE: &str = ".turborel_pass";
/// What separates the hosts of a list, and their ports.
const LIST: char = ',';

/// A setting that cannot be read, or settings no connection can be made
/// by: SQLSTATE 08001, the client unable to connect.
pub(crate) fn invalid(msg: &str) -> Failure {
    Failure::from(&SqlError::new(Code::Connection, msg).with_state(SqlState::UNABLE_TO_CONNECT))
}

/// Where a server is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// At this host name or address, and port, over TCP.
    Tcp(String, u16),
    /// At the Unix socket of this path; on Windows, the named pipe.
    Local(PathBuf),
}

/// The files a connection's TLS is made of.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsFiles {
    /// A PEM file of the authorities trusted.
    pub roots: Option<PathBuf>,
    /// Whether the platform's authorities are trusted too.
    pub system_roots: bool,
    /// A PEM file of the client's certificate chain, to present.
    pub certificate: Option<PathBuf>,
    /// A PEM file of its private key.
    pub key: Option<PathBuf>,
    /// The name the server's certificate is checked against, if not the
    /// host's.
    pub server_name: Option<String>,
}

/// How to connect, as the settings say.
#[derive(Clone)]
pub struct Settings {
    /// The servers, tried in turn.
    pub hosts: Vec<Host>,
    /// The user to connect as.
    pub user: String,
    /// The user's password, if given; else the password file's.
    pub password: Option<String>,
    /// How TLS is used.
    pub tls_mode: TlsMode,
    /// Whether SCRAM binds its channel.
    pub channel_binding: ChannelBinding,
    /// The program's name, as it tells the server.
    pub application_name: Option<String>,
    /// How long each step of connecting waits; none, no limit.
    pub connect_timeout: Option<Duration>,
    /// The password file, if any.
    pub passfile: Option<PathBuf>,
    /// What a server must be.
    pub target: TargetRole,
    /// Whether the servers are tried in order.
    pub load_balance: LoadBalance,
    /// The files TLS is made of.
    pub tls: TlsFiles,
    /// The SCRAM keys of its connections, so a reconnection skips their
    /// derivation; shared by its clones.
    #[cfg(feature = "scram")]
    pub key_cache: crate::scram::KeyCache,
}

impl std::fmt::Debug for Settings {
    /// Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("hosts", &self.hosts)
            .field("user", &self.user)
            .field("tls_mode", &self.tls_mode)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

/// The value of `key` in `options` parsed by `parse`, or `default`.
fn read<T>(options: &Options, key: &str, parse: impl Fn(&str, &str) -> Result<T>, default: T) -> Result<T> {
    options.get(key).map_or(Ok(default), |text| parse(key, text))
}

/// A path setting's value.
fn path(options: &Options, key: &str) -> Option<PathBuf> {
    options.get(key).map(PathBuf::from)
}

/// The port `text` is, the value of `key`.
fn port(key: &str, text: &str) -> Result<u16> {
    text.trim().parse().map_err(|_| invalid(&format!("[DESIGN] {key} is {text}, no port")))
}

/// The hosts `hosts` names, each of its own port, or the port of the
/// same place in `ports`, or the one port `ports` gives, or the default.
fn hosts(hosts: &str, ports: &str) -> Result<Vec<Host>> {
    let ports: Vec<u16> = ports.split(LIST).filter(|p| !p.trim().is_empty()).map(|p| port(PORT, p)).collect::<Result<_>>()?;
    let names: Vec<&str> = hosts.split(LIST).map(str::trim).collect();
    let fallback = |i: usize| ports.get(i).or_else(|| ports.first().filter(|_| ports.len() == 1)).copied().unwrap_or(DEFAULT_PORT);
    names.iter().enumerate().map(|(i, name)| host(name, fallback(i))).collect()
}

/// The host `name` is: a path, of a local socket; `[<address>]` or
/// `[<address>]:<port>`, of IPv6; `<name>:<port>`; or a name, at `port`.
fn host(name: &str, default: u16) -> Result<Host> {
    if name.is_empty() {
        return Err(invalid("[DESIGN] a host of the list is empty"));
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Ok(Host::Local(PathBuf::from(name)));
    }
    if let Some(v6) = name.strip_prefix('[') {
        let (address, after) = v6.split_once(']').ok_or_else(|| invalid(&format!("[DESIGN] {name} has no ]")))?;
        let at = after.strip_prefix(':').map_or(Ok(default), |p| port(HOST, p))?;
        return Ok(Host::Tcp(address.to_owned(), at));
    }
    // A name of two colons or more is an IPv6 address unbracketed, of no
    // port.
    if let Some((h, p)) = name.split_once(':').filter(|(_, p)| !p.contains(':')) {
        return Ok(Host::Tcp(h.to_owned(), port(HOST, p)?));
    }
    Ok(Host::Tcp(name.to_owned(), default))
}

/// The time `text` is, in whole seconds; 0 for no limit.
fn seconds(key: &str, text: &str) -> Result<Option<Duration>> {
    let n: u64 = text.trim().parse().map_err(|_| invalid(&format!("[DESIGN] {key} is {text}, not a whole number of seconds")))?;
    Ok((n > 0).then(|| Duration::from_secs(n)))
}

/// The file `name` in the home directory, if there is one.
fn at_home(name: &str) -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(name))
}

impl Settings {
    /// The settings `explicit` gives, then the service it or the process's
    /// environment names, then the environment, then the defaults.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08001 if a setting cannot be read, a service named cannot
    /// be read, or no user is named.
    pub fn resolve(explicit: &Options) -> Result<Settings> {
        Settings::resolve_in(explicit, |name| std::env::var(name).ok())
    }

    /// The settings of the connection URI `uri`, as `resolve` lays them.
    ///
    /// # Errors
    ///
    /// As `Options::from_uri`'s and `resolve`'s.
    pub fn from_uri(uri: &str) -> Result<Settings> {
        Settings::resolve(&Options::from_uri(uri)?)
    }

    /// The settings as `resolve` lays them, `variable` reading the
    /// environment's variables.
    ///
    /// ```
    /// use turborel_client::settings::{DEFAULT_PORT, Host, Options, Settings, TargetRole};
    ///
    /// let options = Options::from_uri("turborel://ann@db1:7000,db2/?target=read-write")?;
    /// let settings = Settings::resolve_in(&options, |name| (name == "TURBOREL_PORT").then(|| "7500".to_owned()))?;
    /// assert_eq!(settings.hosts, [Host::Tcp("db1".into(), 7000), Host::Tcp("db2".into(), 7500)]);
    /// assert_eq!((settings.user.as_str(), settings.target), ("ann", TargetRole::ReadWrite));
    /// # assert_ne!(DEFAULT_PORT, 7500);
    /// # Ok::<(), turborel_client::Failure>(())
    /// ```
    ///
    /// # Errors
    ///
    /// As `resolve`'s.
    pub fn resolve_in(explicit: &Options, variable: impl Fn(&str) -> Option<String>) -> Result<Settings> {
        let environment = Options::environment(variable);
        let named = explicit.over(&environment);
        let service = match named.get(SERVICE) {
            Some(name) => service(&path(&named, SERVICE_FILE).or_else(|| at_home(DEFAULT_SERVICE_FILE)).unwrap_or_default(), name)?,
            None => Options::new(),
        };
        Settings::of(&explicit.over(&service).over(&environment))
    }

    /// The settings `options` give, laid already, the defaults for the
    /// rest.
    fn of(options: &Options) -> Result<Settings> {
        let user = options.get(USER).ok_or_else(|| invalid("[DESIGN] no user is named: by the program, a service, or TURBOREL_USER"))?;
        Ok(Settings {
            hosts: hosts(options.get(HOST).unwrap_or(DEFAULT_HOST), options.get(PORT).unwrap_or_default())?,
            user: user.to_owned(),
            password: options.get(PASSWORD).map(str::to_owned),
            tls_mode: read(options, TLS_MODE, TlsMode::parse, TlsMode::default())?,
            channel_binding: read(options, CHANNEL_BINDING, ChannelBinding::parse, ChannelBinding::default())?,
            application_name: options.get(APPLICATION_NAME).map(str::to_owned),
            connect_timeout: read(options, CONNECT_TIMEOUT, seconds, Some(CONNECT_WITHIN))?,
            passfile: path(options, PASSFILE).or_else(|| at_home(DEFAULT_PASSFILE)),
            target: read(options, TARGET, TargetRole::parse, TargetRole::default())?,
            load_balance: read(options, LOAD_BALANCE, LoadBalance::parse, LoadBalance::default())?,
            tls: TlsFiles {
                roots: path(options, TLS_ROOTS),
                system_roots: read(options, TLS_SYSTEM_ROOTS, values::boolean, false)?,
                certificate: path(options, TLS_CERTIFICATE),
                key: path(options, TLS_KEY),
                server_name: options.get(SERVER_NAME).map(str::to_owned),
            },
            #[cfg(feature = "scram")]
            key_cache: crate::scram::KeyCache::new(),
        })
    }

    /// The password to prove at `host`: the settings', else the password
    /// file's for `host`, its port and the user, if the file can be read.
    pub fn password_for(&self, host: &Host) -> Option<String> {
        if let Some(password) = &self.password {
            return Some(password.clone());
        }
        let text = files::read_regular(self.passfile.as_deref()?)?;
        let (name, port) = match host {
            Host::Tcp(name, port) => (name.clone(), port.to_string()),
            Host::Local(path) => (path.display().to_string(), DEFAULT_PORT.to_string()),
        };
        files::password(&text, (&name, &port, &self.user))
    }
}

/// The settings of the section `name` of the service file `file`.
///
/// # Errors
///
/// SQLSTATE 08001 if it cannot be read, a line is malformed, or it has no
/// section `name`.
pub fn service(file: &Path, name: &str) -> Result<Options> {
    let text = std::fs::read_to_string(file).map_err(|e| invalid(&format!("[DESIGN] cannot read {}: {e}", file.display())))?;
    files::service(&text, name, file)
}

#[cfg(test)]
mod tests;
