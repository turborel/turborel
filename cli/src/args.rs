//! \[DESIGN\] The command line of `turborel` (RULES.md D46): where the
//! server is, who connects, the SQL to run, and the files to import
//! (D59). Split from lib.rs (RULES.md R-SIZE-4).

use std::path::PathBuf;
use std::time::Duration;

use turborel_client::connection::Password;
use turborel_client::import::{DEFAULT_BATCH, Dialect, Import};
use turborel_client::timeouts::Timeouts;

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
    /// At a Unix socket; on Windows, a named pipe.
    #[cfg(any(unix, windows))]
    Socket(PathBuf),
}

/// How a connection is secured by TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tls {
    /// The PEM file of the certificate authorities trusted, if one is
    /// named.
    pub roots: Option<PathBuf>,
    /// Whether the platform's certificate authorities are trusted too:
    /// only when asked (`--tls-system-roots`).
    pub system: bool,
    /// The name the server's certificate is checked against: the host's,
    /// unless one is named.
    pub name: Option<String>,
    /// The PEM files of the certificate chain the client presents and of
    /// its private key, if it presents one.
    pub identity: Option<(PathBuf, PathBuf)>,
}

/// How the user proves who they are.
#[derive(Debug, Clone)]
pub enum Credentials {
    /// By nothing: trusted, as a server trusts a client of its machine.
    Trust,
    /// By a password, typed at the terminal, proved by SCRAM-SHA-256.
    Ask,
    /// By this password, from the environment, proved by SCRAM-SHA-256.
    Given(Password),
    /// By the certificate presented over TLS (`Tls::identity`).
    Certificate,
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
    /// Rows of a file, imported as `Options::import` says (RULES.md D59).
    Import(PathBuf),
}

/// How the files `--import` names are read, and where their rows go
/// (RULES.md D59).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Importing {
    /// The table, the batches, whether bad rows are skipped.
    pub into: Import,
    /// How the files are written.
    pub dialect: Dialect,
}

/// The formats `--format` names.
const CSV: &str = "csv";
/// Tab-separated text.
const TSV: &str = "tsv";

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
    /// What a null value is shown as, and what a field imported is
    /// written as to be null.
    pub null: String,
    /// How long it waits on the server: to connect and log in, and for
    /// each reply after.
    pub timeouts: Timeouts,
    /// How files are imported, if any are.
    pub import: Option<Importing>,
}

/// What the command line asks for.
#[derive(Debug, Clone)]
pub enum Command {
    /// The usage text.
    Help,
    /// The release, as `turborel::product` gives it.
    Version,
    /// A session.
    Run(Box<Options>),
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
    system: bool,
    name: Option<String>,
    certificate: Option<PathBuf>,
    key: Option<PathBuf>,
    inputs: Vec<Input>,
    stop_on_error: bool,
    null: Option<String>,
    timeouts: Timeouts,
    import: NamedImport,
}

/// What the command line named of an import, before it is checked.
#[derive(Debug, Default)]
struct NamedImport {
    table: Option<String>,
    format: Option<String>,
    header: bool,
    delimiter: Option<String>,
    batch: Option<u64>,
    skip: bool,
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
            "--version" => return Ok(Command::Version),
            "-h" | "--host" => named.host = Some(value()?),
            "-p" | "--port" => named.port = Some(port(&value()?)?),
            "--socket" => named.socket = Some(PathBuf::from(value()?)),
            "-U" | "--user" => named.user = Some(value()?),
            "-W" | "--password" => named.ask = true,
            "--tls" => named.roots = Some(PathBuf::from(value()?)),
            "--tls-system-roots" => named.system = true,
            "--server-name" => named.name = Some(value()?),
            "--certificate" => named.certificate = Some(PathBuf::from(value()?)),
            "--key" => named.key = Some(PathBuf::from(value()?)),
            "-c" | "--command" => named.inputs.push(Input::Command(value()?)),
            "-f" | "--file" => named.inputs.push(Input::File(PathBuf::from(value()?))),
            "--stop-on-error" => named.stop_on_error = true,
            "--null" => named.null = Some(value()?),
            "--connect-timeout" => named.timeouts.connect = seconds(&value()?)?,
            "--read-timeout" => named.timeouts.read = seconds(&value()?)?,
            "--import" => named.inputs.push(Input::Import(PathBuf::from(value()?))),
            "--table" => named.import.table = Some(value()?),
            "--format" => named.import.format = Some(value()?),
            "--header" => named.import.header = true,
            "--delimiter" => named.import.delimiter = Some(value()?),
            "--batch" => named.import.batch = Some(rows(&value()?)?),
            "--skip-bad-rows" => named.import.skip = true,
            other => return Err(format!("{other} is not an option\n\n{}", crate::USAGE)),
        }
    }
    options(named, env).map(|o| Command::Run(Box::new(o)))
}

/// The port `text` names.
fn port(text: &str) -> Result<u16, String> {
    text.parse().map_err(|_| format!("{text} is no port"))
}

/// The limit of time `text` names, in whole seconds: none for 0.
fn seconds(text: &str) -> Result<Option<Duration>, String> {
    let n: u64 = text.parse().map_err(|_| format!("{text} is no number of seconds"))?;
    Ok((n > 0).then(|| Duration::from_secs(n)))
}

/// The number of rows `text` names.
fn rows(text: &str) -> Result<u64, String> {
    text.parse().map_err(|_| format!("{text} is no number of rows"))
}

/// How the files `inputs` names are imported, if it names any, as
/// `named` says, null written `null`: an import's options only with one,
/// and its table always.
fn importing(inputs: &[Input], named: &NamedImport, null: &str) -> Result<Option<Importing>, String> {
    if !inputs.iter().any(|i| matches!(i, Input::Import(_))) {
        let any = named.table.is_some() || named.format.is_some() || named.delimiter.is_some() || named.batch.is_some();
        if any || named.header || named.skip {
            return Err(
                "--table, --format, --header, --delimiter, --batch and --skip-bad-rows are of an import: name --import FILE too".into()
            );
        }
        return Ok(None);
    }
    let Some(table) = &named.table else { return Err("--import needs the table its rows go into: --table NAME".into()) };
    let format = named.format.as_deref().unwrap_or(CSV);
    let mut dialect = if format.eq_ignore_ascii_case(CSV) {
        Dialect::csv()
    } else if format.eq_ignore_ascii_case(TSV) {
        Dialect::tsv()
    } else {
        return Err(format!("{format} is no format: {CSV} or {TSV}"));
    };
    if let Some(text) = &named.delimiter {
        let mut chars = text.chars();
        let (Some(one), None) = (chars.next(), chars.next()) else { return Err(format!("--delimiter is one character, not {text:?}")) };
        dialect = dialect.with_delimiter(one)?;
    }
    if named.header {
        dialect = dialect.with_header();
    }
    let into = Import { batch: named.batch.unwrap_or(DEFAULT_BATCH), skip: named.skip, ..Import::into(table) };
    Ok(Some(Importing { into, dialect: dialect.with_null(null) }))
}

/// The options `named` makes, checked.
fn options(named: Named, env: &dyn Fn(&str) -> Option<String>) -> Result<Options, String> {
    let null = named.null.clone().unwrap_or_default();
    let import = importing(&named.inputs, &named.import, &null)?;
    let user = named.user.or_else(|| env("USER")).or_else(|| env("USERNAME")).ok_or_else(|| "name the user: --user NAME".to_owned())?;
    let identity = match (named.certificate, named.key) {
        (Some(certificate), Some(key)) => Some((certificate, key)),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => return Err("--certificate and --key name a certificate and its key: both".into()),
    };
    let credentials = match (named.ask, env(PASSWORD_VARIABLE), identity.is_some()) {
        (true, _, _) => Credentials::Ask,
        (false, Some(password), _) => Credentials::Given(Password(password)),
        (false, None, true) => Credentials::Certificate,
        (false, None, false) => Credentials::Trust,
    };
    let secured = named.roots.is_some() || named.system;
    if (named.name.is_some() || identity.is_some()) && !secured {
        return Err("--server-name, --certificate and --key are of TLS: name --tls or --tls-system-roots too".into());
    }
    let tls = secured.then_some(Tls { roots: named.roots, system: named.system, name: named.name, identity });
    let server = server(named.socket, named.host, named.port, tls)?;
    let inputs = if named.inputs.is_empty() { vec![Input::Standard] } else { named.inputs };
    let (stop_on_error, timeouts) = (named.stop_on_error, named.timeouts);
    Ok(Options { server, user, credentials, inputs, stop_on_error, null, timeouts, import })
}

/// Where the server is: at the Unix socket or named pipe `socket`, if one
/// is named, and nothing else of where; else at `host`'s `port`, over `tls` if given.
fn server(socket: Option<PathBuf>, host: Option<String>, port: Option<u16>, tls: Option<Tls>) -> Result<Server, String> {
    let Some(path) = socket else {
        return Ok(Server::Tcp { host: host.unwrap_or_else(|| DEFAULT_HOST.to_owned()), port: port.unwrap_or(DEFAULT_PORT), tls });
    };
    if host.is_some() || port.is_some() || tls.is_some() {
        return Err("--socket names where the server is: not with --host, --port or TLS".into());
    }
    socket_server(path)
}

/// The server at the Unix socket `path`, on Unix; at the named pipe
/// `path`, on Windows.
#[cfg(any(unix, windows))]
#[expect(clippy::unnecessary_wraps, reason = "elsewhere than Unix and Windows, it fails")]
pub(crate) fn socket_server(path: PathBuf) -> Result<Server, String> {
    Ok(Server::Socket(path))
}

/// None elsewhere than Unix and Windows: neither Unix sockets nor named
/// pipes are there.
#[cfg(not(any(unix, windows)))]
pub(crate) fn socket_server(_: PathBuf) -> Result<Server, String> {
    Err("--socket: a Unix socket is reached on Unix only, a named pipe on Windows only".into())
}
