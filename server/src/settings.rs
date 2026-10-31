//! \[DESIGN\] The server's settings (RULES.md D36, D42): declared on the
//! database it serves, as a plug-in's are, so they are set as any setting
//! is, kept with the database's definitions, and listed with the others.
//! The server reads each when it needs it: where it listens, TLS, trust
//! and passwords' iterations as it starts, recording the values it started
//! with (`mark_started`); TLS's files, what clients prove themselves by,
//! TCP keepalive's times and the longest message, as connections begin;
//! its limits as statements and connections begin, and its time to stop
//! as it is told to, so a change of one takes effect with no restart. One
//! that cannot be read is read as its default.

use std::num::NonZeroUsize;

use turborel::db::Database;
use turborel::error::Result;
use turborel::settings::{Definition, Effect, Kind, OFF, SWITCHES};
use turborel::value::{Decimal, Value};
use turborel_protocol::{DEFAULT_PORT, MAX_BODY};

/// Where TCP connections are taken: hosts or addresses, by commas.
pub const HOST: &str = "SERVER.HOST";
/// The TCP port connections are taken at; 0 for one the system chooses.
pub const PORT: &str = "SERVER.PORT";
/// The Unix socket connections are taken at, on Windows the named pipe,
/// if not empty.
pub const SOCKET: &str = "SERVER.SOCKET";
/// Whether TCP connections may, or must, use TLS: OFF, ON or REQUIRED.
pub const TLS: &str = "SERVER.TLS";
/// The PEM file of the server's certificate chain, its own first.
pub const TLS_CERTIFICATE: &str = "SERVER.TLS_CERTIFICATE";
/// The PEM file of the server's private key.
pub const TLS_KEY: &str = "SERVER.TLS_KEY";
/// The words of `TLS`: none, offered, or required of TCP clients.
pub const TLS_MODES: &[&str] = &["OFF", "ON", "REQUIRED"];
/// The PEM file of the authorities whose certificates a TLS client may
/// present, to log in by them (`certificate.rs`); empty, none is asked for.
pub const TLS_CLIENT_CA: &str = "SERVER.TLS_CLIENT_CA";
/// Whether SCRAM-SHA-256 is taken over plain TCP: ON or OFF.
pub const SCRAM_WITHOUT_TLS: &str = "SERVER.SCRAM_WITHOUT_TLS";
/// Whether a client of this machine is trusted as who it says: ON or OFF.
pub const TRUST_LOCAL: &str = "SERVER.TRUST_LOCAL";
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
/// The longest message taken from a client, in bytes, read as its
/// connection is taken.
pub const MAX_MESSAGE_BYTES: &str = "SERVER.MAX_MESSAGE_BYTES";
/// The prepared statements a connection holds at once, at most.
pub const MAX_STATEMENTS: &str = "SERVER.MAX_STATEMENTS";
/// \[DESIGN\] The default of `MAX_STATEMENTS`.
pub const DEFAULT_MAX_STATEMENTS: usize = 1024;
/// About how many bytes of rows go in one batch.
pub const BATCH_BYTES: &str = "SERVER.BATCH_BYTES";
/// The milliseconds a connection has, from when it is taken, to be secured
/// and authenticated, the two together; 0 for no limit.
pub const LOGIN_TIMEOUT_MS: &str = "SERVER.LOGIN_TIMEOUT_MS";
/// \[DESIGN\] The default of `LOGIN_TIMEOUT_MS`: a minute, as PostgreSQL's
/// authentication_timeout.
pub const DEFAULT_LOGIN_TIMEOUT_MS: usize = 60_000;
/// The milliseconds the server waits on a client, for its next request or
/// to take what it is sent, before it closes the connection; 0 for no
/// limit.
pub const IDLE_TIMEOUT_MS: &str = "SERVER.IDLE_TIMEOUT_MS";
/// The milliseconds a server told to stop lets its connections finish
/// their statements, before it cancels them and closes the connections; 0
/// for no limit.
pub const SHUTDOWN_TIMEOUT_MS: &str = "SERVER.SHUTDOWN_TIMEOUT_MS";
/// \[DESIGN\] The default of `SHUTDOWN_TIMEOUT_MS`: half a minute, so a
/// statement near its end finishes, and a stop asked by a service manager
/// ends before it gives up and kills the server.
pub const DEFAULT_SHUTDOWN_TIMEOUT_MS: usize = 30_000;
/// The milliseconds the server waits for the next request of a client
/// whose transaction is active, before it closes the connection, the
/// transaction rolled back and its locks let go; 0 for no limit.
pub const IDLE_IN_TRANSACTION_TIMEOUT_MS: &str = "SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS";
/// Authentications checked at once, at most: apart from `WORKERS`, so
/// clients logging in never hold the workers statements need.
pub const MAX_LOGINS: &str = "SERVER.MAX_LOGINS";
/// \[DESIGN\] The most `MAX_LOGINS`: the blocking threads a server's
/// runtime allows them, beside `MAX_WORKERS`.
pub const MOST_LOGINS: usize = 64;
/// \[DESIGN\] The default of `MAX_LOGINS`: a few, so a flood of logins
/// takes no more than a few processors' time, and each is checked in the
/// time its hashing takes.
pub const DEFAULT_MAX_LOGINS: usize = 4;
/// The longest, in milliseconds, a client's login waits after failed ones
/// from its address; 0 for no wait.
pub const LOGIN_DELAY_MS: &str = "SERVER.LOGIN_DELAY_MS";
/// \[DESIGN\] The default of `LOGIN_DELAY_MS`: ten seconds, a sixth of
/// `DEFAULT_LOGIN_TIMEOUT_MS`, so a delayed client still has time to log in.
pub const DEFAULT_LOGIN_DELAY_MS: usize = 10_000;
/// The milliseconds a TCP connection is idle before the system probes
/// whether its client is still there; 0 for the system's own.
pub const KEEPALIVE_IDLE_MS: &str = "SERVER.KEEPALIVE_IDLE_MS";
/// The milliseconds between the system's probes of a TCP connection's
/// client, unanswered; 0 for the system's own.
pub const KEEPALIVE_INTERVAL_MS: &str = "SERVER.KEEPALIVE_INTERVAL_MS";
/// The probes unanswered after which the system closes a TCP connection;
/// 0 for the system's own.
pub const KEEPALIVE_PROBES: &str = "SERVER.KEEPALIVE_PROBES";
/// \[DESIGN\] The most of `KEEPALIVE_IDLE_MS` and `KEEPALIVE_INTERVAL_MS`:
/// Linux's most, 32767 seconds (`MAX_TCP_KEEPIDLE` and
/// `MAX_TCP_KEEPINTVL` in its `include/net/tcp.h`).
const MOST_KEEPALIVE_MS: i128 = 32_767_000;
/// \[DESIGN\] The most `KEEPALIVE_PROBES`: Linux's most
/// (`MAX_TCP_KEEPCNT` in its `include/net/tcp.h`); Windows takes 255.
const MOST_KEEPALIVE_PROBES: i128 = 127;

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

/// A whole-number setting from `min` to `max`, read as each connection is
/// taken.
fn connected(name: &str, (min, max): (i128, i128), default: usize, doc: &str) -> Definition {
    setting(name, Kind::Integer { min, max }, whole(default), Effect::Connection, doc)
}

/// A whole-number setting from `min` to `max`, read as it is needed.
fn limit(name: &str, (min, max): (i128, i128), default: usize, doc: &str) -> Definition {
    setting(name, Kind::Integer { min, max }, whole(default), Effect::Statement, doc)
}

/// The server's settings, as declared.
fn definitions() -> Vec<Definition> {
    let mut all = Vec::from(started());
    all.extend(limits());
    all.extend(waits());
    all.extend(proofs());
    all.extend(kept_alive());
    all
}

/// TCP keepalive's times, read as each connection is taken
/// (`keepalive.rs`): a change taken for the connections after.
fn kept_alive() -> [Definition; 3] {
    [
        connected(
            KEEPALIVE_IDLE_MS,
            (0, MOST_KEEPALIVE_MS),
            0,
            "The milliseconds a TCP connection is idle before the system probes whether its client is still there, rounded up to whole seconds; 0 for the system's own.",
        ),
        connected(
            KEEPALIVE_INTERVAL_MS,
            (0, MOST_KEEPALIVE_MS),
            0,
            "The milliseconds between the system's probes of a TCP connection's client, unanswered, rounded up to whole seconds; 0 for the system's own.",
        ),
        connected(
            KEEPALIVE_PROBES,
            (0, MOST_KEEPALIVE_PROBES),
            0,
            "The probes unanswered after which the system closes a TCP connection, its transaction rolled back; 0 for the system's own.",
        ),
    ]
}

/// TLS's files and what clients prove themselves by, read as each
/// connection is secured or authenticated: a change taken for the
/// connections after (`tls.rs`).
fn proofs() -> [Definition; 4] {
    let text = Kind::Text { max: MAX_TEXT };
    [
        setting(
            TLS_CERTIFICATE,
            text.clone(),
            Value::Char(String::new()),
            Effect::Connection,
            "The PEM file of the server's certificate chain, taken again for the connections after a change.",
        ),
        setting(
            TLS_KEY,
            text.clone(),
            Value::Char(String::new()),
            Effect::Connection,
            "The PEM file of the server's private key, taken again for the connections after a change.",
        ),
        setting(
            TLS_CLIENT_CA,
            text,
            Value::Char(String::new()),
            Effect::Connection,
            "The PEM file of the authorities whose certificates a TLS client may present to log in by; none asked for if empty.",
        ),
        setting(
            SCRAM_WITHOUT_TLS,
            Kind::Choice(SWITCHES),
            Value::Char(OFF.into()),
            Effect::Connection,
            "Whether SCRAM-SHA-256 is taken over plain TCP (ON), where an eavesdropper may guess the password from the exchange, or only over TLS or a Unix socket (OFF).",
        ),
    ]
}

/// The settings read as the server starts: where it listens, TLS, who it
/// trusts, and its passwords' iterations.
fn started() -> [Definition; 6] {
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
        setting(SOCKET, text, Value::Char(String::new()), Effect::Start, "The Unix socket or named pipe taken at, unless empty."),
        setting(
            TLS,
            Kind::Choice(TLS_MODES),
            Value::Char("OFF".into()),
            Effect::Start,
            "Whether TCP connections may use TLS (ON), or must (REQUIRED).",
        ),
        setting(
            TRUST_LOCAL,
            Kind::Choice(SWITCHES),
            Value::Char(OFF.into()),
            Effect::Start,
            "Whether a client of this machine is trusted as who it says (ON), or proves it as every other does (OFF).",
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
fn limits() -> [Definition; 6] {
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
        setting(
            MAX_MESSAGE_BYTES,
            Kind::Integer { min: MIN_MESSAGE_BYTES, max: most },
            whole(usize::try_from(MAX_BODY).unwrap_or(usize::MAX)),
            Effect::Connection,
            "The longest message, in bytes, the server takes from a client, read as the client's connection is taken.",
        ),
        limit(BATCH_BYTES, (1, most), DEFAULT_BATCH_BYTES, "About how many bytes of rows the server sends in one batch."),
        limit(
            MAX_LOGINS,
            (1, i128::try_from(MOST_LOGINS).unwrap_or(most)),
            DEFAULT_MAX_LOGINS,
            "Authentications the server checks at once, at most, apart from its statements' workers.",
        ),
    ]
}

/// The waits' limits, in milliseconds, read as they begin.
fn waits() -> [Definition; 5] {
    let most = i128::from(u32::MAX);
    [
        limit(
            LOGIN_TIMEOUT_MS,
            (0, most),
            DEFAULT_LOGIN_TIMEOUT_MS,
            "The milliseconds a connection has, from when it is taken, to begin TLS and authenticate, the two together, before it is closed; 0 for no limit.",
        ),
        limit(
            IDLE_TIMEOUT_MS,
            (0, most),
            0,
            "The milliseconds the server waits on a client, for its next request or to take what it is sent, before it closes the connection, its transaction rolled back; 0 for no limit.",
        ),
        limit(
            SHUTDOWN_TIMEOUT_MS,
            (0, most),
            DEFAULT_SHUTDOWN_TIMEOUT_MS,
            "The milliseconds a server told to stop lets each connection finish its statement, before it cancels the statements and closes the connections, their transactions rolled back; 0 for no limit.",
        ),
        limit(
            IDLE_IN_TRANSACTION_TIMEOUT_MS,
            (0, most),
            0,
            "The milliseconds the server waits for the next request of a client in a transaction before it closes the connection, the transaction rolled back and its locks let go; 0 for no limit.",
        ),
        limit(
            LOGIN_DELAY_MS,
            (0, most),
            DEFAULT_LOGIN_DELAY_MS,
            "The longest, in milliseconds, a login waits after failed ones from its client's address; 0 for no wait.",
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

/// `db`'s value of the setting `name`, declared by `declare`; its default
/// if it cannot be read, as when the database is closed, so that a limit
/// is never quietly another than the one declared.
fn value(db: &Database, name: &str) -> Option<Value> {
    db.setting(name).ok().or_else(|| definitions().into_iter().find(|d| d.name == name).map(|d| d.default))
}

/// `db`'s value of the whole-number setting `name`, declared by `declare`:
/// its default if it cannot be read (`value`); 0 if it is no setting of
/// the server's.
pub fn count(db: &Database, name: &str) -> usize {
    match value(db, name) {
        Some(Value::Exact(d)) => usize::try_from(d.units).unwrap_or(usize::MAX),
        Some(
            Value::Approx(_)
            | Value::Char(_)
            | Value::Null
            | Value::Bits(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Extension(_),
        )
        | None => 0,
    }
}

/// `db`'s value of the setting `name`, declared by `declare`, as
/// milliseconds: none if it is 0, no limit.
pub fn millis(db: &Database, name: &str) -> Option<std::time::Duration> {
    let n = count(db, name);
    (n > 0).then(|| std::time::Duration::from_millis(u64::try_from(n).unwrap_or(u64::MAX)))
}

/// `db`'s value of the text setting `name`, declared by `declare`: its
/// default if it cannot be read (`value`); empty if it is no setting of
/// the server's.
pub fn text(db: &Database, name: &str) -> String {
    match value(db, name) {
        Some(Value::Char(s)) => s,
        Some(
            Value::Exact(_)
            | Value::Approx(_)
            | Value::Null
            | Value::Bits(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Extension(_),
        )
        | None => String::new(),
    }
}

/// Records `db`'s values of the server's settings read as it starts as
/// those it started with, so INFORMATION_SCHEMA.SETTINGS shows one set
/// since as waiting for a restart (RULES.md D36): as it starts listening.
///
/// # Errors
///
/// `Database::settings_started`'s.
pub fn mark_started(db: &Database) -> Result<()> {
    let names: Vec<String> = definitions().into_iter().filter(|d| d.effect == Effect::Start).map(|d| d.name).collect();
    db.settings_started(&names.iter().map(String::as_str).collect::<Vec<_>>())
}

// Unit tests: the settings read when they cannot be (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D36, D42: a setting of the server's that
    /// cannot be read, as of a database it was never declared on, is read
    /// as its default, and one that is none of the server's as nothing.
    /// Regression test (R-TEST-7): it was read as 1, which made the
    /// server's limits, as of connections, 1.
    #[test]
    fn a_setting_not_read_is_its_default() {
        let db = Database::new();
        assert_eq!(count(&db, MAX_CONNECTIONS), DEFAULT_MAX_CONNECTIONS);
        assert_eq!(millis(&db, LOGIN_TIMEOUT_MS), Some(std::time::Duration::from_secs(60_u64)));
        assert_eq!(text(&db, HOST), DEFAULT_HOST);
        assert_eq!((count(&db, "SERVER.NO_SUCH"), text(&db, "SERVER.NO_SUCH")), (0, String::new()));
        declare(&db).unwrap();
        db.set_setting(MAX_CONNECTIONS, &whole(7)).unwrap();
        assert_eq!(count(&db, MAX_CONNECTIONS), 7);
    }
}
