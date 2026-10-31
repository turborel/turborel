//! \[DESIGN\] Where a server listens (RULES.md D42), as its database's
//! settings say when it starts: SERVER.HOST and SERVER.PORT for TCP, with
//! TLS as SERVER.TLS says; and SERVER.SOCKET for a Unix socket, on Windows
//! a named pipe, which needs no TLS: only this machine reaches it.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;

use turborel::db::Database;
use turborel::error::{Code, Result, SqlError};

use crate::server::{Endpoint, Protocol};
use crate::settings::{self, HOST, PORT, SOCKET, TLS};
use crate::tls::{Files, Tls};
use crate::transport::{Listener, Tcp};

fn unable(msg: String) -> SqlError {
    SqlError::new(Code::Io, msg)
}

/// The addresses of `hosts`, by commas, each an IP address, one in
/// brackets, or a host name, at `port`.
///
/// # Errors
///
/// SQLCODE -915 if a name names no address.
pub fn addresses(hosts: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let mut found = Vec::new();
    for host in hosts.split(',').map(str::trim).filter(|h| !h.is_empty()) {
        let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
        if let Ok(ip) = bare.parse::<IpAddr>() {
            found.push(SocketAddr::new(ip, port));
            continue;
        }
        let named: Vec<SocketAddr> =
            (bare, port).to_socket_addrs().map_err(|e| unable(format!("[DESIGN] cannot listen at {host}: {e}")))?.collect();
        if named.is_empty() {
            return Err(unable(format!("[DESIGN] {host} names no address")));
        }
        found.extend(named);
    }
    Ok(found)
}

/// The TLS SERVER.TLS asks for, of the files SERVER.TLS_CERTIFICATE,
/// SERVER.TLS_KEY and SERVER.TLS_CLIENT_CA name, taken again as they
/// change (`Tls::reload`); none if it is OFF.
///
/// # Errors
///
/// SQLCODE -915 if a file is not named, cannot be read, or is not what it
/// should be.
pub fn tls(db: &Database) -> Result<Option<Arc<Tls>>> {
    let mode = settings::text(db, TLS);
    if mode == "OFF" {
        return Ok(None);
    }
    let files = Files::of(db);
    if files.chain.is_empty() || files.key.is_empty() {
        return Err(unable(format!("[DESIGN] SERVER.TLS is {mode}, so SERVER.TLS_CERTIFICATE and SERVER.TLS_KEY name its files")));
    }
    // Read once, as TLS is taken, and shown while it is (RULES.md D56).
    let secured = Tls::from_files(&files, mode == "REQUIRED").and_then(|t| t.of(db));
    Ok(Some(Arc::new(secured.map_err(|e| unable(format!("[DESIGN] cannot take TLS: {e}")))?)))
}

/// The endpoints `db`'s settings ask for, listening, each served by
/// `protocol`.
///
/// # Errors
///
/// SQLCODE -915 if one cannot listen, TLS cannot be taken, or the
/// settings name nowhere to listen; -910 if the database is closed.
pub async fn bind(db: &Database, protocol: &Arc<dyn Protocol>) -> Result<Vec<Endpoint>> {
    settings::mark_started(db)?;
    let port = u16::try_from(settings::count(db, PORT)).map_err(|_| unable("[DESIGN] SERVER.PORT is no port".into()))?;
    let secured = tls(db)?;
    let mut endpoints = Vec::new();
    for address in addresses(&settings::text(db, HOST), port)? {
        let tcp =
            Tcp::bind(address).await.map_err(|e| unable(format!("[DESIGN] cannot listen at {address}: {e}")))?.kept_alive_by(db.clone());
        endpoints.push(Endpoint { listener: Box::new(tcp), protocol: Arc::clone(protocol), tls: secured.clone() });
    }
    let socket = settings::text(db, SOCKET);
    if !socket.is_empty() {
        endpoints.push(Endpoint { listener: local(db, &socket).await?, protocol: Arc::clone(protocol), tls: None });
    }
    if endpoints.is_empty() {
        return Err(unable("[DESIGN] SERVER.HOST and SERVER.SOCKET name nowhere to listen".into()));
    }
    Ok(endpoints)
}

/// A listener at the Unix socket `path`, shown among `db`'s files while
/// it listens (RULES.md D56).
#[cfg(unix)]
async fn local(db: &Database, path: &str) -> Result<Box<dyn Listener>> {
    let bound = crate::transport::Local::bind(Path::new(path)).await;
    let bound = bound.map_err(|e| unable(format!("[DESIGN] cannot listen at {path}: {e}")))?;
    Ok(Box::new(bound.noting(db.note_file(Path::new(path), true)?)))
}

/// A listener at the named pipe `name`, on Windows, shown among `db`'s
/// files while it listens (RULES.md D56).
#[cfg(windows)]
#[expect(clippy::unused_async, reason = "awaited as on Unix; the pipe is made within the runtime, as it must be")]
async fn local(db: &Database, name: &str) -> Result<Box<dyn Listener>> {
    let bound = crate::transport::Pipe::bind(name).map_err(|e| unable(format!("[DESIGN] cannot listen at {name}: {e}")))?;
    Ok(Box::new(bound.noting(db.note_file(Path::new(name), true)?)))
}

/// No local transport elsewhere than Unix and Windows.
#[cfg(not(any(unix, windows)))]
#[expect(clippy::unused_async, reason = "awaited as on Unix, where it listens")]
async fn local(_: &Database, _: &str) -> Result<Box<dyn Listener>> {
    Err(unable("[DESIGN] SERVER.SOCKET: neither Unix sockets nor named pipes are on this platform".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D42: SERVER.HOST names addresses by
    /// commas, spaces around them and empty ones ignored: IPv4 and IPv6
    /// addresses, an IPv6 one in brackets, a host name at each of its
    /// addresses; each at the port; a name with no address is refused.
    #[test]
    fn hosts_name_addresses_at_the_port() {
        let found = addresses(" 127.0.0.1, ::1 ,[::1],,", 9).unwrap();
        assert_eq!(found, ["127.0.0.1:9".parse().unwrap(), "[::1]:9".parse().unwrap(), "[::1]:9".parse().unwrap()]);
        assert_eq!(addresses("", 9).unwrap(), Vec::<SocketAddr>::new());
        let local = addresses("localhost", 9).unwrap();
        assert!(!local.is_empty() && local.iter().all(|a| a.ip().is_loopback() && a.port() == 9), "{local:?}");
        assert_eq!(addresses("a:b", 9).err().map(|e| e.code), Some(Code::Io), "no lookup, so no network");
    }
}
