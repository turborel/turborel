//! \[DESIGN\] Where a server listens (RULES.md D42), as its database's
//! settings say when it starts: SERVER.HOST and SERVER.PORT for TCP, with
//! TLS as SERVER.TLS says; and SERVER.SOCKET for a Unix socket, which
//! needs no TLS: only this machine reaches it.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;

use turborel::db::Database;
use turborel::error::{Code, Result, SqlError};

use crate::server::{Endpoint, Protocol};
use crate::settings::{self, HOST, PORT, SOCKET, TLS, TLS_CERTIFICATE, TLS_KEY};
use crate::tls::Tls;
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

/// The TLS SERVER.TLS asks for, of the files SERVER.TLS_CERTIFICATE and
/// SERVER.TLS_KEY name; none if it is OFF.
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
    let (chain, key) = (settings::text(db, TLS_CERTIFICATE), settings::text(db, TLS_KEY));
    if chain.is_empty() || key.is_empty() {
        return Err(unable(format!("[DESIGN] SERVER.TLS is {mode}, so SERVER.TLS_CERTIFICATE and SERVER.TLS_KEY name its files")));
    }
    let secured = Tls::from_files(Path::new(&chain), Path::new(&key), mode == "REQUIRED");
    Ok(Some(Arc::new(secured.map_err(|e| unable(format!("[DESIGN] cannot take TLS: {e}")))?)))
}

/// The endpoints `db`'s settings ask for, listening, each served by
/// `protocol`.
///
/// # Errors
///
/// SQLCODE -915 if one cannot listen, TLS cannot be taken, or the
/// settings name nowhere to listen.
pub async fn bind(db: &Database, protocol: &Arc<dyn Protocol>) -> Result<Vec<Endpoint>> {
    let port = u16::try_from(settings::count(db, PORT)).map_err(|_| unable("[DESIGN] SERVER.PORT is no port".into()))?;
    let secured = tls(db)?;
    let mut endpoints = Vec::new();
    for address in addresses(&settings::text(db, HOST), port)? {
        let tcp = Tcp::bind(address).await.map_err(|e| unable(format!("[DESIGN] cannot listen at {address}: {e}")))?;
        endpoints.push(Endpoint { listener: Box::new(tcp), protocol: Arc::clone(protocol), tls: secured.clone() });
    }
    let socket = settings::text(db, SOCKET);
    if !socket.is_empty() {
        endpoints.push(Endpoint { listener: local(&socket).await?, protocol: Arc::clone(protocol), tls: None });
    }
    if endpoints.is_empty() {
        return Err(unable("[DESIGN] SERVER.HOST and SERVER.SOCKET name nowhere to listen".into()));
    }
    Ok(endpoints)
}

/// A listener at the Unix socket `path`.
#[cfg(unix)]
async fn local(path: &str) -> Result<Box<dyn Listener>> {
    let bound = crate::transport::Local::bind(Path::new(path)).await;
    Ok(Box::new(bound.map_err(|e| unable(format!("[DESIGN] cannot listen at {path}: {e}")))?))
}

/// No Unix socket, off Unix.
#[cfg(not(unix))]
#[expect(clippy::unused_async, reason = "awaited as on Unix, where it listens")]
async fn local(_: &str) -> Result<Box<dyn Listener>> {
    Err(unable("[DESIGN] SERVER.SOCKET: Unix sockets are not on this platform".into()))
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
