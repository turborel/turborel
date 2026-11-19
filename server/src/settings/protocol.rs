//! \[DESIGN\] The native protocol's settings (RULES.md D194, D196): the
//! shortest frame sent compressed, read as each connection is granted
//! compression, and the log position a session waits for before each
//! statement, a session's own to set.

use turborel::db::Session;
use turborel::settings::{Definition, Effect, Kind};
use turborel::value::Value;
use turborel_protocol::MAX_BODY;
use turborel_protocol::compress::DEFAULT_THRESHOLD;

use super::{connected, whole};

/// \[DESIGN\] The bytes of the shortest frame sent compressed to a client
/// granted compression; 0, compression is not granted (RULES.md D196).
pub const COMPRESSION_MIN_BYTES: &str = "SERVER.COMPRESSION_MIN_BYTES";
/// \[DESIGN\] The log position a session's statements wait for its
/// database to have applied before they run, as a replica's session reads
/// what a commit on the primary made, bounded by STATEMENT.TIMEOUT_MS; 0,
/// none (RULES.md D194).
pub const READ_AFTER: &str = "REPLICATION.READ_AFTER";

/// The native protocol's settings, as `declare` declares them.
pub(super) fn definitions() -> [Definition; 2] {
    let read_after = Definition {
        name: READ_AFTER.into(),
        kind: Kind::Integer { min: 0, max: i128::from(i64::MAX) },
        default: whole(0),
        per_session: true,
        node: false,
        effect: Effect::Statement,
        doc: "The log position, as a commit's Complete gave it, that a session's statements wait for the database to have applied before they run, within STATEMENT.TIMEOUT_MS; 0 for none.".into(),
    };
    [
        connected(
            COMPRESSION_MIN_BYTES,
            (0, i128::from(MAX_BODY)),
            DEFAULT_THRESHOLD,
            "The bytes of the shortest frame sent compressed to a client that asked for compression, read as it connects; 0 for no compression.",
        ),
        read_after,
    ]
}

/// The position `session`'s statements wait for (`READ_AFTER`): 0 for
/// none, or if the setting cannot be read.
pub fn read_after(session: &Session) -> u64 {
    let Ok(Value::Exact(d)) = session.setting(READ_AFTER) else { return 0 };
    u64::try_from(d.units).unwrap_or(0)
}
