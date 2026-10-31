//! \[DESIGN\] The connections CONNECT makes (RULES.md D46): a `<SQL-server
//! name>` [ISO/IEC 9075:1992 §15.1] is an address, 'host', 'host:port' or,
//! on Unix, a socket's path beginning '/', on Windows a named pipe's name
//! beginning '\\' (as '\\.\pipe\turborel'); DEFAULT is the server the
//! command line names. A connection is proved as the command line says: a
//! password typed is asked for again, for each. Split from lib.rs
//! (RULES.md R-SIZE-4).

use std::path::PathBuf;

use turborel::error::{Code, SqlError, SqlState};
use turborel_client::Failure;
use turborel_client::agent::{Connected, Connector};

use crate::args::{DEFAULT_PORT, Options, Server, Tls, socket_server};
use crate::{Unreached, mechanism, reach};

/// What reads a password typed.
pub type Ask = Box<dyn FnMut() -> std::io::Result<String>>;

/// \[DESIGN\] What begins the name of a server of this machine: a Unix
/// socket's absolute path; on Windows, a named pipe's name.
#[cfg(not(windows))]
const LOCAL_PREFIX: &str = "/";
/// \[DESIGN\] What begins the name of a server of this machine: a Unix
/// socket's absolute path; on Windows, a named pipe's name.
#[cfg(windows)]
const LOCAL_PREFIX: &str = r"\\";

/// The command line's connector: to the servers CONNECT names.
pub struct Dialer {
    options: Options,
    ask: Ask,
}

impl Dialer {
    /// A connector as `options` say, a password typed read by `ask`.
    pub fn new(options: Options, ask: Ask) -> Dialer {
        Dialer { options, ask }
    }

    /// The server `address` names: secured by TLS as the default server
    /// is, its certificate checked against the host named.
    fn server(&self, address: &str) -> Result<Server, String> {
        if address.starts_with(LOCAL_PREFIX) {
            return socket_server(PathBuf::from(address));
        }
        let tls = match &self.options.server {
            Server::Tcp { tls, .. } => tls.as_ref().map(|t| Tls { roots: t.roots.clone(), name: None, identity: t.identity.clone() }),
            #[cfg(any(unix, windows))]
            Server::Socket(_) => None,
        };
        let (host, port) = host_port(address)?;
        Ok(Server::Tcp { host, port, tls })
    }
}

/// The host and port of `address`: 'host', 'host:port', or an IPv6
/// address, in brackets if a port follows.
fn host_port(address: &str) -> Result<(String, u16), String> {
    let (host, port) = match address.strip_prefix('[') {
        Some(rest) => {
            let (host, after) = rest.split_once(']').ok_or_else(|| format!("'{address}' has no closing ']'"))?;
            match after {
                "" => (host, None),
                _ => (host, Some(after.strip_prefix(':').ok_or_else(|| format!("'{address}': a ':' and a port follow ']'"))?)),
            }
        }
        // An IPv6 address without its brackets has colons of its own.
        None => match address.split_once(':') {
            Some((host, port)) => {
                if port.contains(':') {
                    (address, None)
                } else {
                    (host, Some(port))
                }
            }
            None => (address, None),
        },
    };
    if host.is_empty() {
        return Err(format!("'{address}' names no host"));
    }
    let port = port.map(|p| p.parse().map_err(|_| format!("'{p}' is no port"))).transpose()?.unwrap_or(DEFAULT_PORT);
    Ok((host.to_owned(), port))
}

/// `why` a server cannot be reached, as a connection failure.
fn unreachable(why: &str) -> Failure {
    Failure::from(&SqlError::new(Code::Connection, format!("[DESIGN] {why}")).with_state(SqlState::CONNECTION_FAILURE))
}

impl Connector for Dialer {
    fn connect(&mut self, server: Option<&str>, user: Option<&str>) -> turborel_client::connection::Result<Connected> {
        let server = match server {
            Some(address) => self.server(address).map_err(|why| unreachable(&why))?,
            None => self.options.server.clone(),
        };
        let mut mechanism = mechanism(&self.options.credentials, &mut self.ask).map_err(|why| unreachable(&why))?;
        reach(&server, (user.unwrap_or(&self.options.user), mechanism.as_mut()), self.options.timeouts).map_err(|unreached| match unreached
        {
            Unreached::Address(why) => unreachable(&why),
            Unreached::Refused(failure) => failure,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D46: a server's address is a host and
    /// an optional port, an IPv6 one in brackets if a port follows.
    #[test]
    fn an_address_names_a_host_and_port() {
        assert_eq!(host_port("db.example:7000"), Ok(("db.example".to_owned(), 7000)));
        assert_eq!(host_port("db.example"), Ok(("db.example".to_owned(), DEFAULT_PORT)));
        assert_eq!(host_port("[::1]:7000"), Ok(("::1".to_owned(), 7000)));
        assert_eq!(host_port("[::1]"), Ok(("::1".to_owned(), DEFAULT_PORT)));
        assert_eq!(host_port("::1"), Ok(("::1".to_owned(), DEFAULT_PORT)));
        assert!(host_port("[::1").is_err());
        assert!(host_port("[::1]7000").is_err());
        assert!(host_port(":7000").is_err());
        assert!(host_port("h:port").is_err());
    }
}
