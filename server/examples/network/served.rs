//! The servers the network benchmark measures, and connections to them:
//! one in the clear, at TCP and a Unix socket; one with TLS required. Both
//! hold the same tables and the user ANN, proved by SCRAM-SHA-256.

use std::net::{SocketAddr, TcpStream};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

use rustls::ClientConfig;
use turborel::db::Database;
use turborel::value::{Decimal, Value};
use turborel_client::connection::{self, Connection, Mechanism, Trust};
use turborel_client::scram::Scram;
use turborel_client::tls::{self, TlsStream};
use turborel_server::auth::Authenticator;
use turborel_server::password::Pbkdf2;
use turborel_server::scram::Scram as Scrammed;
use turborel_server::settings::{self, MAX_CONNECTIONS, SCRAM_WITHOUT_TLS, SOCKET};
use turborel_test_support::server::{Running, start_with};
use turborel_test_support::tls::{Pki, pki, secured};

/// Rows of the large result.
pub const BIG_ROWS: usize = 100_000;
/// The text of each of its rows: 64 characters.
const BIG_TEXT: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-.";
/// SCRAM's iterations, the least SERVER.PASSWORD_ITERATIONS takes, so
/// the client's key derivation does not hide the exchange.
const ITERATIONS: u32 = 10_000;
/// The password of ANN.
pub const PASSWORD: &str = "ann's";
/// Connections each server takes, past those the benchmark opens at once.
const CONNECTIONS: i128 = 2_000;

/// A transport the benchmark measures.
#[derive(Clone, Copy)]
pub enum Kind {
    Tcp,
    Unix,
    Tls,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Tcp, Kind::Unix, Kind::Tls];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Tcp => "tcp",
            Kind::Unix => "unix",
            Kind::Tls => "tls",
        }
    }
}

/// The two servers, and what reaches them.
pub struct Served {
    pub clear: Running,
    pub secure: Running,
    pub socket: PathBuf,
    pub config: Arc<ClientConfig>,
    _pki: Pki,
}

/// A connection over any of the transports.
pub enum Any {
    Tcp(Connection<TcpStream>),
    Unix(Connection<std::os::unix::net::UnixStream>),
    Tls(Box<Connection<TlsStream>>),
}

/// Runs `$body` with `$c` the connection inside `$any`, whatever its kind.
#[macro_export]
macro_rules! with {
    ($any:expr, $c:ident => $body:expr) => {
        match $any {
            $crate::served::Any::Tcp($c) => $body,
            $crate::served::Any::Unix($c) => $body,
            $crate::served::Any::Tls($c) => $body,
        }
    };
}

/// `db`'s tables, its user ANN, and the SCRAM authenticators for ANN.
fn furnished(db: &Database) -> Vec<Arc<dyn Authenticator>> {
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE ONE (K INTEGER NOT NULL) CREATE TABLE BIG (K INTEGER NOT NULL, V VARCHAR(100) NOT NULL) CREATE TABLE B (K INTEGER NOT NULL, V VARCHAR(100) NOT NULL)").unwrap();
    let session = db.session("X");
    session.direct("INSERT INTO ONE VALUES (1)").unwrap();
    let insert = session.prepare("INSERT INTO BIG VALUES (?, ?)").unwrap();
    for k in 0..BIG_ROWS {
        session.execute_prepared(&insert, &[Value::Exact(Decimal::new(k as i128, 0)), Value::Char(BIG_TEXT.to_owned())]).unwrap();
    }
    session.commit().unwrap();
    let made = Pbkdf2::new(NonZeroU32::new(ITERATIONS).unwrap());
    db.create_user("ANN", made.make(PASSWORD).unwrap().as_bytes()).unwrap();
    session.direct("GRANT SELECT ON ONE TO ANN").unwrap();
    session.commit().unwrap();
    let scram = Scrammed::new(db.clone(), &made).unwrap();
    vec![Arc::new(scram.plus()), Arc::new(scram)]
}

impl Served {
    pub fn start() -> Served {
        let pki = pki("bench-network");
        let clear = Database::new();
        settings::declare(&clear).unwrap();
        let socket = pki.scratch.socket("s");
        clear.set_setting(SOCKET, &Value::Char(socket.display().to_string())).unwrap();
        clear.set_setting(SCRAM_WITHOUT_TLS, &Value::Char("ON".into())).unwrap();
        let clear_running = start_with(&clear, &[(MAX_CONNECTIONS, CONNECTIONS)], furnished(&clear));
        let secure = Database::new();
        secured(&secure, &pki, "REQUIRED");
        let secure_running = start_with(&secure, &[(MAX_CONNECTIONS, CONNECTIONS)], furnished(&secure));
        let config = tls::trusting(&pki.authority).unwrap();
        Served { clear: clear_running, secure: secure_running, socket, config, _pki: pki }
    }

    /// A connection of `kind` as `user`, by `mechanism`.
    pub fn connect_as(&self, kind: Kind, user: &str, mechanism: &mut dyn Mechanism) -> Any {
        match kind {
            Kind::Tcp => Any::Tcp(connection::connect(self.clear.address, user, mechanism).unwrap()),
            Kind::Unix => Any::Unix(connection::connect_local(&self.socket, user, mechanism).unwrap()),
            Kind::Tls => Any::Tls(Box::new(tls::connect(self.secure.address, ("localhost", &self.config), user, mechanism).unwrap())),
        }
    }

    /// A connection of `kind` as X, trusted.
    pub fn connect(&self, kind: Kind) -> Any {
        self.connect_as(kind, "X", &mut Trust)
    }

    /// A connection of `kind` as ANN, by SCRAM-SHA-256.
    pub fn connect_scram(&self, kind: Kind) -> Any {
        self.connect_as(kind, "ANN", &mut Scram::new(PASSWORD).in_the_clear())
    }

    /// The TCP address of the server of `kind`.
    pub fn address(&self, kind: Kind) -> SocketAddr {
        match kind {
            Kind::Tcp | Kind::Unix => self.clear.address,
            Kind::Tls => self.secure.address,
        }
    }
}
