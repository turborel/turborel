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

/// A sink that keeps every event, as `Debug` shows it.
#[derive(Clone, Default)]
pub struct Kept(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl turborel::diagnostics::Diagnostics for Kept {
    fn event(&self, event: &turborel::diagnostics::Event) {
        self.0.lock().unwrap().push(format!("{event:?}"));
    }
}

impl Kept {
    /// The events kept whose `Debug` begins with `kind`.
    pub fn of(&self, kind: &str) -> Vec<String> {
        self.0.lock().unwrap().iter().filter(|e| e.starts_with(kind)).cloned().collect()
    }

    /// Every event kept.
    pub fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}
