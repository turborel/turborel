//! \[DESIGN\] The server's settings (RULES.md D36, D42): declared on the
//! database it serves, as a plug-in's are, so they are set as any setting
//! is, kept with the database's definitions, and listed with the others.
//! The server reads each when it needs it: where it listens, TLS, trust
//! and passwords' iterations as it starts; its limits as statements and
//! connections begin, so a change of one takes effect with no restart.

use std::num::NonZeroUsize;

use turborel::db::Database;
use turborel::error::Result;
use turborel::settings::{Definition, Effect, Kind};
use turborel::value::{Decimal, Value};
use turborel_protocol::{DEFAULT_PORT, MAX_BODY};

/// Where TCP connections are taken: hosts or addresses, by commas.
pub const HOST: &str = "SERVER.HOST";
/// The TCP port connections are taken at; 0 for one the system chooses.
pub const PORT: &str = "SERVER.PORT";
/// The Unix socket connections are taken at, if not empty (Unix only).
pub const SOCKET: &str = "SERVER.SOCKET";
/// Whether TCP connections may, or must, use TLS: OFF, ON or REQUIRED.
pub const TLS: &str = "SERVER.TLS";
/// The PEM file of the server's certificate chain, its own first.
pub const TLS_CERTIFICATE: &str = "SERVER.TLS_CERTIFICATE";
/// The PEM file of the server's private key.
pub const TLS_KEY: &str = "SERVER.TLS_KEY";
/// The words of `TLS`: none, offered, or required of TCP clients.
pub const TLS_MODES: &[&str] = &["OFF", "ON", "REQUIRED"];
/// Whether a client of this machine is trusted as who it says: LOCAL or
/// OFF.
pub const TRUST: &str = "SERVER.TRUST";
/// The words of `TRUST`.
pub const TRUST_MODES: &[&str] = &["LOCAL", "OFF"];
/// The PBKDF2 iterations of a password's verifier made from the server's
/// start on.
pub const PASSWORD_ITERATIONS: &str = "SERVER.PASSWORD_ITERATIONS";
/// \[DESIGN\] The default of `PASSWORD_ITERATIONS`: OWASP's least for
/// PBKDF2-HMAC-SHA256.
pub const DEFAULT_PASSWORD_ITERATIONS: usize = 600_000;
/// \[DESIGN\] The fewest `PASSWORD_ITERATIONS`: for tests, and machines
/// too slow for more; below it, a password is too quickly guessed.
const MIN_PASSWORD_ITERATIONS: i128 = 10_000;
/// Statements run at once, at most: the workers.
pub const WORKERS: &str = "SERVER.WORKERS";
/// Connections open at once, at most.
pub const MAX_CONNECTIONS: &str = "SERVER.MAX_CONNECTIONS";
/// The longest message taken from a client, in bytes.
pub const MAX_MESSAGE_BYTES: &str = "SERVER.MAX_MESSAGE_BYTES";
/// The prepared statements a connection holds at once, at most.
pub const MAX_STATEMENTS: &str = "SERVER.MAX_STATEMENTS";
/// \[DESIGN\] The default of `MAX_STATEMENTS`.
pub const DEFAULT_MAX_STATEMENTS: usize = 1024;
/// About how many bytes of rows go in one batch.
pub const BATCH_BYTES: &str = "SERVER.BATCH_BYTES";
/// The milliseconds a connection has to be secured and authenticated; 0
/// for no limit.
pub const LOGIN_TIMEOUT: &str = "SERVER.LOGIN_TIMEOUT";
/// \[DESIGN\] The default of `LOGIN_TIMEOUT`: a minute, as PostgreSQL's
/// authentication_timeout.
pub const DEFAULT_LOGIN_TIMEOUT: usize = 60_000;
/// The milliseconds the server waits on a client, for its next request or
/// to take what it is sent, before it closes the connection; 0 for no
/// limit.
pub const IDLE_TIMEOUT: &str = "SERVER.IDLE_TIMEOUT";

/// \[DESIGN\] The most workers: the blocking threads a server's runtime
/// should allow.
pub const MAX_WORKERS: usize = 1024;
/// \[DESIGN\] The default host: this machine only, so a server is reached
/// from others only once its operator says so.
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// \[DESIGN\] The default limit of connections.
pub const DEFAULT_MAX_CONNECTIONS: usize = 256;
/// \[DESIGN\] The default batch of rows: about 64 KiB.
pub const DEFAULT_BATCH_BYTES: usize = 64 * 1024;
/// \[DESIGN\] The shortest limit of messages: room for any message but a
/// statement or a row.
const MIN_MESSAGE_BYTES: i128 = 1024;
/// \[DESIGN\] The longest text of a host list or a socket's path.
const MAX_TEXT: usize = 1024;

fn whole(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// A setting of the server's: not a session's.
fn setting(name: &str, kind: Kind, default: Value, effect: Effect, doc: &str) -> Definition {
    Definition { name: name.into(), kind, default, per_session: false, effect, doc: doc.into() }
}

/// A whole-number setting from `min` to `max`, read as it is needed.
fn limit(name: &str, (min, max): (i128, i128), default: usize, doc: &str) -> Definition {
    setting(name, Kind::Integer { min, max }, whole(default), Effect::Statement, doc)
}

/// The server's settings, as declared.
fn definitions() -> Vec<Definition> {
    let mut all = Vec::from(started());
    all.extend(limits());
    all
}

/// The settings read as the server starts: where it listens, TLS, who it
/// trusts, and its passwords' iterations.
fn started() -> [Definition; 8] {
    let text = Kind::Text { max: MAX_TEXT };
    [
        setting(
            HOST,
            text.clone(),
            Value::Char(DEFAULT_HOST.into()),
            Effect::Start,
            "Hosts or addresses, by commas, where TCP connections are taken.",
        ),
        setting(
            PORT,
            Kind::Integer { min: 0, max: i128::from(u16::MAX) },
            whole(usize::from(DEFAULT_PORT)),
            Effect::Start,
            "The TCP port connections are taken at.",
        ),
        setting(SOCKET, text.clone(), Value::Char(String::new()), Effect::Start, "The Unix socket connections are taken at, unless empty."),
        setting(
            TLS,
            Kind::Choice(TLS_MODES),
            Value::Char("OFF".into()),
            Effect::Start,
            "Whether TCP connections may use TLS (ON), or must (REQUIRED).",
        ),
        setting(
            TLS_CERTIFICATE,
            text.clone(),
            Value::Char(String::new()),
            Effect::Start,
            "The PEM file of the server's certificate chain.",
        ),
        setting(TLS_KEY, text, Value::Char(String::new()), Effect::Start, "The PEM file of the server's private key."),
        setting(
            TRUST,
            Kind::Choice(TRUST_MODES),
            Value::Char("OFF".into()),
            Effect::Start,
            "Whether a client of this machine is trusted as who it says (LOCAL), or proves it as every other does (OFF).",
        ),
        setting(
            PASSWORD_ITERATIONS,
            Kind::Integer { min: MIN_PASSWORD_ITERATIONS, max: i128::from(u32::MAX) },
            whole(DEFAULT_PASSWORD_ITERATIONS),
            Effect::Start,
            "The PBKDF2 iterations of a password's verifier made from the start on.",
        ),
    ]
}

/// The limits, read as statements and connections begin.
fn limits() -> [Definition; 7] {
    let processors = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let most = i128::from(u32::MAX);
    [
        limit(
            WORKERS,
            (1, i128::try_from(MAX_WORKERS).unwrap_or(most)),
            processors.min(MAX_WORKERS),
            "Statements the server runs at once, at most.",
        ),
        limit(MAX_CONNECTIONS, (1, most), DEFAULT_MAX_CONNECTIONS, "Connections the server takes at once, at most."),
        limit(MAX_STATEMENTS, (1, most), DEFAULT_MAX_STATEMENTS, "Prepared statements a connection holds at once, at most."),
        limit(
            MAX_MESSAGE_BYTES,
            (MIN_MESSAGE_BYTES, most),
            usize::try_from(MAX_BODY).unwrap_or(usize::MAX),
            "The longest message, in bytes, the server takes from a client.",
        ),
        limit(BATCH_BYTES, (1, most), DEFAULT_BATCH_BYTES, "About how many bytes of rows the server sends in one batch."),
        limit(
            LOGIN_TIMEOUT,
            (0, most),
            DEFAULT_LOGIN_TIMEOUT,
            "The milliseconds a connection has to begin TLS and authenticate before it is closed; 0 for no limit.",
        ),
        limit(
            IDLE_TIMEOUT,
            (0, most),
            0,
            "The milliseconds the server waits on a client, for its next request or to take what it is sent, before it closes the connection, its transaction rolled back; 0 for no limit.",
        ),
    ]
}

/// Declares the server's settings on `db`, those it has not declared
/// already: a database may be served again, by another server.
///
/// # Errors
///
/// `Database::declare_setting`'s.
pub fn declare(db: &Database) -> Result<()> {
    for definition in definitions() {
        if db.setting(&definition.name).is_err() {
            db.declare_setting(definition)?;
        }
    }
    Ok(())
}

/// `db`'s value of the whole-number setting `name`, declared by `declare`;
/// 1 if it cannot be read, as when the database is closed.
pub fn count(db: &Database, name: &str) -> usize {
    match db.setting(name) {
        Ok(Value::Exact(d)) => usize::try_from(d.units).unwrap_or(usize::MAX),
        Ok(
            Value::Approx(_)
            | Value::Char(_)
            | Value::Null
            | Value::Bits(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Extension(_),
        )
        | Err(_) => 1,
    }
}

/// `db`'s value of the setting `name`, declared by `declare`, as
/// milliseconds: none if it is 0, no limit.
pub fn millis(db: &Database, name: &str) -> Option<std::time::Duration> {
    let n = count(db, name);
    (n > 0).then(|| std::time::Duration::from_millis(u64::try_from(n).unwrap_or(u64::MAX)))
}

/// `db`'s value of the text setting `name`, declared by `declare`; empty if
/// it cannot be read.
pub fn text(db: &Database, name: &str) -> String {
    match db.setting(name) {
        Ok(Value::Char(s)) => s,
        Ok(
            Value::Exact(_)
            | Value::Approx(_)
            | Value::Null
            | Value::Bits(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Extension(_),
        )
        | Err(_) => String::new(),
    }
}
