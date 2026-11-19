//! Why the server closes a connection it waited on: its idle time, or
//! any other limit, run out, or the server hurrying to stop. The
//! connection's task and a worker waiting on its client itself tell the
//! same (`native/lent.rs`, RULES.md D232).

use std::time::Duration;

use turborel::error::SqlError;
use turborel_protocol::broken;

/// What the server waited on a client for, told once its idle time runs out.
pub(crate) const IDLE_WAITING: &str = "with nothing from or to the client";

/// The connection went past the setting `name`, `limit`, having waited
/// `waiting`: closed.
pub(crate) fn went_past(name: &str, limit: Duration, waiting: &str) -> SqlError {
    broken(format!("[DESIGN] the connection went past {name}, {} ms, {waiting}: closed", limit.as_millis()))
}

/// The server hurries to stop: the connection is closed.
pub(crate) fn hurrying() -> SqlError {
    broken("[DESIGN] the server is stopping, past SERVER.SHUTDOWN_TIMEOUT_MS or told again: closed")
}
