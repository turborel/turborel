//! \[DESIGN\] `turborel`, the command-line client of the turborel server
//! (RULES.md D46): SQL given on its command line, read from files or typed,
//! run over a connection (`turborel_client`), one statement at a time, each
//! a `<direct SQL statement>` [ISO/IEC 9075:1992 §20.1]; a query's rows
//! shown as a table as they come. A library, `main.rs` its one caller, so
//! tests reach all of it.

mod args;
mod interrupt;
mod run;
mod show;
mod split;

use std::net::ToSocketAddrs;

use turborel::error::SqlState;

use turborel_client::connection::{Connection, Mechanism, Password, Trust};
use turborel_client::target::{Duplex, Target, connect_to};
use turborel_client::tls;

/// A failure, as the server reports it.
pub use turborel_client::Failure;

pub use args::{Command, Credentials, DEFAULT_HOST, DEFAULT_PORT, Input, Options, PASSWORD_VARIABLE, Server, Tls, parse};
pub use interrupt::{Handlers, Interrupts};
pub use run::{Io, MORE, PROMPT, run};
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
  -U, --user NAME        who connects (default the user of the environment)
  -W, --password         prove it by a password, typed without its echo
  -c, --command SQL      run SQL's statements; more than once for more
  -f, --file FILE        run FILE's statements; more than once for more
      --stop-on-error    end the run at the first statement that fails
      --null TEXT        show a null value as TEXT (default nothing)
      --help             print this

Without -c or -f, statements are read from standard input: typed at a terminal,
each is run once a semicolon ends it, and the end of input (Ctrl-D; Ctrl-Z then
Enter on Windows) quits. The last statement of a file needs no semicolon.
A password may be given in the environment variable TURBOREL_PASSWORD instead;
without either, the server is asked to trust the user, as it may a client of
its own machine. A password is sent only over TLS or a Unix socket.
Ctrl-C cancels the statement running. The program exits with 1 if any
statement failed.";

/// Connects as `options` say, a password typed read by `typed`.
///
/// # Errors
///
/// Why it cannot: the server cannot be reached, or refuses the user.
pub fn connect(options: &Options, typed: impl FnOnce() -> std::io::Result<String>) -> Result<Connection<Box<dyn Duplex>>, String> {
    let mut mechanism: Box<dyn Mechanism> = match &options.credentials {
        Credentials::Trust => Box::new(Trust),
        Credentials::Ask => Box::new(Password(typed().map_err(|e| format!("cannot read the password: {e}"))?)),
        Credentials::Given(password) => Box::new(password.clone()),
    };
    let mut last = None;
    for target in targets(&options.server)? {
        match connect_to(&target, &options.user, mechanism.as_mut()) {
            Ok(connection) => return Ok(connection),
            Err(failure) => {
                // A refusal is the server's answer: no other address is tried.
                if failure.sqlstate != SqlState::CONNECTION_FAILURE.as_str() {
                    return Err(failure.to_string());
                }
                last = Some(failure);
            }
        }
    }
    Err(last.map_or_else(|| "the host has no address".to_owned(), |failure: Failure| failure.to_string()))
}

/// Each way to reach `server`, in the order tried.
fn targets(server: &Server) -> Result<Vec<Target>, String> {
    match server {
        Server::Tcp { host, port, tls } => {
            let addresses = (host.as_str(), *port).to_socket_addrs().map_err(|e| format!("cannot find {host}: {e}"))?;
            let Some(secured) = tls else { return Ok(addresses.map(Target::Tcp).collect()) };
            let config = tls::trusting(&secured.roots).map_err(|e| e.to_string())?;
            let name = secured.name.clone().unwrap_or_else(|| host.clone());
            Ok(addresses.map(|address| Target::Tls(address, name.clone(), config.clone())).collect())
        }
        #[cfg(unix)]
        Server::Socket(path) => Ok(vec![Target::Local(path.clone())]),
    }
}
