//! \[DESIGN\] `turborel`, the command-line client of the turborel server
//! (RULES.md D46): SQL given on its command line, read from files or typed,
//! run over a connection (`turborel_client`), one statement at a time, each
//! a `<direct SQL statement>` [ISO/IEC 9075:1992 §20.1]; a query's rows
//! shown as a table as they come; files' rows imported into tables
//! (RULES.md D59). A library, `main.rs` its one caller, so
//! tests reach all of it.

mod args;
mod dial;
mod interrupt;
mod run;
mod show;
mod split;

use std::net::ToSocketAddrs;

use turborel::error::SqlState;

use turborel_client::agent::Connected;
use turborel_client::connection::{Certificate, Mechanism, Trust};
use turborel_client::scram::Scram;
use turborel_client::target::{Target, connect_to_within};
use turborel_client::timeouts::Timeouts;
use turborel_client::tls;

/// A failure, as the server reports it.
pub use turborel_client::Failure;

pub use args::{Command, Credentials, DEFAULT_HOST, DEFAULT_PORT, Importing, Input, Options, PASSWORD_VARIABLE, Server, Tls, parse};
pub use dial::Dialer;
pub use interrupt::{Handlers, Interrupts};
pub use run::{FileSource, Io, MORE, PROMPT, Statements, run};
pub use show::{MAX_WIDTH, Table};
pub use split::Splitter;

/// The usage text.
pub const USAGE: &str = "usage: turborel [options]

  -h, --host HOST        the server's host (default 127.0.0.1)
  -p, --port PORT        its port (default 7422)
      --socket PATH      the server's Unix socket, instead (Unix only)
      --tls FILE         over TLS, trusting the certificates of the PEM file FILE
      --server-name NAME the name the server's certificate is checked against
                         (default the host)
      --certificate FILE over TLS, present the certificate chain of the PEM file FILE
      --key FILE         and its private key, of the PEM file FILE
  -U, --user NAME        who connects (default the user of the environment)
  -W, --password         prove it by a password, typed without its echo
  -c, --command SQL      run SQL's statements; more than once for more
  -f, --file FILE        run FILE's statements; more than once for more
      --stop-on-error    end the run at the first statement that fails
      --null TEXT        show a null value as TEXT, and import a field TEXT as
                         null (default nothing)
      --import FILE      insert FILE's rows into the table --table names; more
                         than once for more, in turn with -c and -f
      --table NAME       the table rows are imported into
      --format FORMAT    how FILE is written: csv (default) or tsv
      --header           FILE's first line names the columns of its fields
      --delimiter CHAR   the character between fields (default a comma for csv,
                         a tab for tsv)
      --batch ROWS       commit each ROWS rows imported (default 1000; 0 for all
                         at once)
      --skip-bad-rows    tell a row that fails, and import the rest
      --connect-timeout SECONDS
                         wait at most SECONDS to connect and log in (default 30;
                         0 for no limit)
      --read-timeout SECONDS
                         wait at most SECONDS for each reply of the server
                         (default 0, no limit)
      --help             print this
      --version          print the release

Without -c or -f, statements are read from standard input: typed at a terminal,
each is run once a semicolon ends it, and the end of input (Ctrl-D; Ctrl-Z then
Enter on Windows) quits. The last statement of a file needs no semicolon.
A password may be given in the environment variable TURBOREL_PASSWORD instead;
it is proved by SCRAM-SHA-256, never sent, and only over TLS or a Unix socket:
over plain TCP the client refuses to begin. Without a password, a certificate
presented proves the user its common name names; without either, the server is
asked to trust the user, as one with SERVER.TRUST LOCAL does a client of its own
machine.
An import begins with no transaction active; a row that fails stops it, its
batch rolled back, and says the record and line. Ctrl-C cancels the statement
running. The program exits with 1 if any statement or row failed.";

/// How the end of typed input is given: Ctrl-D, or on Windows Ctrl-Z then
/// Enter.
#[cfg(not(windows))]
const END_OF_INPUT: &str = "Ctrl-D";
#[cfg(windows)]
const END_OF_INPUT: &str = "Ctrl-Z then Enter";

/// What the program prints as a typed session begins (RULES.md D46): the
/// client and its licence, the server `connection` reached as `options`
/// say, as whom, and how to end.
pub fn banner(options: &Options, connection: &Connected) -> String {
    let at = match &options.server {
        Server::Tcp { host, port, tls: Some(_) } => format!("at {host}:{port} over TLS"),
        Server::Tcp { host, port, tls: None } => format!("at {host}:{port}"),
        #[cfg(unix)]
        Server::Socket(path) => format!("at the socket {}", path.display()),
    };
    format!(
        "{}, the command-line client\nCopyright 2026, Turborel contributors.\nDistributed under the BSD 2-Clause License.\n\n\
         Connected to {} {at}, as {}.\nEnd each statement with a semicolon; end of input ({END_OF_INPUT}) quits.\n",
        turborel::product(),
        connection.server(),
        connection.authorization(),
    )
}

/// Connects as `options` say, a password typed read by `typed`.
///
/// # Errors
///
/// Why it cannot: the server cannot be reached, or refuses the user.
pub fn connect(options: &Options, typed: impl FnOnce() -> std::io::Result<String>) -> Result<Connected, String> {
    let mut mechanism = mechanism(&options.credentials, typed)?;
    reach(&options.server, (&options.user, mechanism.as_mut()), options.timeouts).map_err(|unreached| match unreached {
        Unreached::Address(why) => why,
        Unreached::Refused(failure) => failure.to_string(),
    })
}

/// How `credentials` are proved, a password typed read by `typed`.
fn mechanism(credentials: &Credentials, typed: impl FnOnce() -> std::io::Result<String>) -> Result<Box<dyn Mechanism>, String> {
    Ok(match credentials {
        Credentials::Trust => Box::new(Trust),
        Credentials::Ask => Box::new(Scram::new(&typed().map_err(|e| format!("cannot read the password: {e}"))?)),
        Credentials::Given(password) => Box::new(Scram::new(&password.0)),
        Credentials::Certificate => Box::new(Certificate),
    })
}

/// Why a server was not reached.
enum Unreached {
    /// Where it is cannot be found.
    Address(String),
    /// It cannot be connected to, or refuses.
    Refused(Failure),
}

/// Connects to `server` as `user`, proved by `mechanism`, waiting on it as
/// `timeouts` say: each of its addresses tried in turn, until one is
/// connected to or refuses. If none is reached, the failure says why of
/// each: one address's, as a TLS handshake failed, is not hidden by
/// another's, as one nothing listens at.
fn reach(server: &Server, (user, mechanism): (&str, &mut dyn Mechanism), timeouts: Timeouts) -> Result<Connected, Unreached> {
    let mut failures: Vec<(String, Failure)> = Vec::new();
    for target in targets(server).map_err(Unreached::Address)? {
        match connect_to_within(&target, user, mechanism, timeouts) {
            Ok(connection) => return Ok(connection),
            Err(failure) => {
                // A refusal is the server's answer: no other address is tried.
                if failure.sqlstate != SqlState::CONNECTION_FAILURE.as_str() {
                    return Err(Unreached::Refused(failure));
                }
                failures.push((address_of(&target), failure));
            }
        }
    }
    let Some((_, first)) = failures.first() else { return Err(Unreached::Address("the host has no address".to_owned())) };
    if failures.len() == 1 {
        return Err(Unreached::Refused(first.clone()));
    }
    let each: Vec<String> = failures.iter().map(|(at, f)| format!("at {at}: {}", f.message)).collect();
    Err(Unreached::Refused(Failure { message: each.join("; "), ..first.clone() }))
}

/// The address `target` reaches, as shown.
fn address_of(target: &Target) -> String {
    match target {
        Target::Tcp(address) => address.to_string(),
        #[cfg(unix)]
        Target::Local(path) => path.display().to_string(),
        Target::Tls(address, ..) => address.to_string(),
    }
}

/// Each way to reach `server`, in the order tried.
fn targets(server: &Server) -> Result<Vec<Target>, String> {
    match server {
        Server::Tcp { host, port, tls } => {
            let addresses = (host.as_str(), *port).to_socket_addrs().map_err(|e| format!("cannot find {host}: {e}"))?;
            let Some(secured) = tls else { return Ok(addresses.map(Target::Tcp).collect()) };
            let config = match &secured.identity {
                Some((chain, key)) => tls::presenting(&secured.roots, (chain, key)),
                None => tls::trusting(&secured.roots),
            }
            .map_err(|e| e.to_string())?;
            let name = secured.name.clone().unwrap_or_else(|| host.clone());
            Ok(addresses.map(|address| Target::Tls(address, name.clone(), config.clone())).collect())
        }
        #[cfg(unix)]
        Server::Socket(path) => Ok(vec![Target::Local(path.clone())]),
    }
}
