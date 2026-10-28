//! Fixtures and helpers shared by the server's tests.

use turborel_client::connection::{self, Connection, Trust};
use turborel_client::tls;

#[cfg(unix)]
pub use turborel_test_support::Scratch;
pub use turborel_test_support::client::{done, rows, state};
pub use turborel_test_support::server::{Running, set, start, start_with};
pub use turborel_test_support::tls::{Pki, pki, secured};

/// A connection over TLS to `server` as X, trusting `pki`'s authority.
pub fn over_tls(server: &Running, pki: &Pki, name: &str) -> connection::Result<Connection<tls::TlsStream>> {
    let config = tls::trusting(&pki.authority).unwrap();
    tls::connect(server.address, (name, &config), "X", &mut Trust)
}
