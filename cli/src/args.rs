//! \[DESIGN\] The command line of `turborel` (RULES.md D46): where the
//! server is, who connects, and the SQL to run. Split from lib.rs (RULES.md
//! R-SIZE-4).

use std::path::PathBuf;

use turborel_client::connection::Password;

/// The host connected to, unless one is named.
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// The port connected to, unless one is named: the server's own default.
pub const DEFAULT_PORT: u16 = 7422;
/// The environment variable a password may be given in.
pub const PASSWORD_VARIABLE: &str = "TURBOREL_PASSWORD";

/// Where the server is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    /// At a host's port, over TCP; over TLS if `tls` says how.
    Tcp {
        /// The host's name or address.
        host: String,
        /// The port.
        port: u16,
        /// TLS, if the connection is secured.
        tls: Option<Tls>,
    },
    /// At a Unix socket.
    #[cfg(unix)]
    Socket(PathBuf),
}

/// How a connection is secured by TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tls {
    /// The PEM file of the certificates trusted, and only them.
    pub roots: PathBuf,
    /// The name the server's certificate is checked against: the host's,
    /// unless one is named.
    pub name: Option<String>,
}

/// How the user proves who they are.
#[derive(Debug, Clone)]
pub enum Credentials {
    /// By nothing: trusted, as a server trusts a client of its machine.
    Trust,
    /// By a password, typed at the terminal.
    Ask,
    /// By this password, from the environment.
    Given(Password),
}

/// SQL to run, in the order named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// Statements given on the command line.
    Command(String),
    /// Statements in a file.
    File(PathBuf),
    /// Statements read from standard input, typed if it is a terminal.
    Standard,
}

/// What a session is to do.
#[derive(Debug, Clone)]
pub struct Options {
    /// Where the server is.
    pub server: Server,
    /// Who connects.
    pub user: String,
    /// How they prove it.
    pub credentials: Credentials,
    /// What to run.
    pub inputs: Vec<Input>,
    /// Whether the first statement that fails ends a run not typed.
    pub stop_on_error: bool,
    /// What a null value is shown as.
    pub null: String,
}

/// What the command line asks for.
#[derive(Debug, Clone)]
pub enum Command {
    /// The usage text.
    Help,
    /// A session.
    Run(Options),
}

/// What the command line named, before it is checked.
#[derive(Debug, Default)]
struct Named {
    host: Option<String>,
    port: Option<u16>,
    socket: Option<PathBuf>,
    user: Option<String>,
    ask: bool,
    roots: Option<PathBuf>,
    name: Option<String>,
    inputs: Vec<Input>,
    stop_on_error: bool,
    null: Option<String>,
}

/// What the command line `args`, without the program's name, asks for;
/// `env` gives the environment's variables.
///
/// # Errors
///
/// What is wrong with it, for the user.
pub fn parse(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<Command, String> {
    let mut named = Named::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--help" => return Ok(Command::Help),
            "-h" | "--host" => named.host = Some(value()?),
            "-p" | "--port" => named.port = Some(port(&value()?)?),
            "--socket" => named.socket = Some(PathBuf::from(value()?)),
            "-U" | "--user" => named.user = Some(value()?),
            "-W" | "--password" => named.ask = true,
            "--tls" => named.roots = Some(PathBuf::from(value()?)),
            "--server-name" => named.name = Some(value()?),
            "-c" | "--command" => named.inputs.push(Input::Command(value()?)),
            "-f" | "--file" => named.inputs.push(Input::File(PathBuf::from(value()?))),
            "--stop-on-error" => named.stop_on_error = true,
            "--null" => named.null = Some(value()?),
            other => return Err(format!("{other} is not an option\n\n{}", crate::USAGE)),
        }
    }
    options(named, env).map(Command::Run)
}

/// The port `text` names.
fn port(text: &str) -> Result<u16, String> {
    text.parse().map_err(|_| format!("{text} is no port"))
}

/// The options `named` makes, checked.
fn options(named: Named, env: &dyn Fn(&str) -> Option<String>) -> Result<Options, String> {
    let user = named.user.or_else(|| env("USER")).or_else(|| env("USERNAME")).ok_or_else(|| "name the user: --user NAME".to_owned())?;
    let credentials = match (named.ask, env(PASSWORD_VARIABLE)) {
        (true, _) => Credentials::Ask,
        (false, Some(password)) => Credentials::Given(Password(password)),
        (false, None) => Credentials::Trust,
    };
    if named.name.is_some() && named.roots.is_none() {
        return Err("--server-name names the server TLS checks: name --tls too".into());
    }
    let server = server(named.socket, named.host, named.port, named.roots.map(|roots| Tls { roots, name: named.name }))?;
    let inputs = if named.inputs.is_empty() { vec![Input::Standard] } else { named.inputs };
    Ok(Options { server, user, credentials, inputs, stop_on_error: named.stop_on_error, null: named.null.unwrap_or_default() })
}

/// Where the server is: at the Unix socket `socket`, if one is named, and
/// nothing else of where; else at `host`'s `port`, over `tls` if given.
fn server(socket: Option<PathBuf>, host: Option<String>, port: Option<u16>, tls: Option<Tls>) -> Result<Server, String> {
    let Some(path) = socket else {
        return Ok(Server::Tcp { host: host.unwrap_or_else(|| DEFAULT_HOST.to_owned()), port: port.unwrap_or(DEFAULT_PORT), tls });
    };
    if host.is_some() || port.is_some() || tls.is_some() {
        return Err("--socket names where the server is: not with --host, --port or --tls".into());
    }
    socket_server(path)
}

/// The server at the Unix socket `path`, on Unix.
#[cfg(unix)]
#[expect(clippy::unnecessary_wraps, reason = "elsewhere than Unix, it fails")]
pub(crate) fn socket_server(path: PathBuf) -> Result<Server, String> {
    Ok(Server::Socket(path))
}

/// The server at the Unix socket `path`, on Unix.
#[cfg(not(unix))]
pub(crate) fn socket_server(_: PathBuf) -> Result<Server, String> {
    Err("--socket: a Unix socket is reached on Unix only".into())
}
