//! \[DESIGN\] The native wire protocol, version 1 (RULES.md D42, D45): its
//! messages and their encoding, without I/O, so the server and the client
//! share one codec and tests can check it byte by byte.
//!
//! A connection begins with the client's Hello: the protocol version, the
//! user, a mechanism of authentication with its first bytes, and the
//! capabilities the client has. The server answers each step of the
//! exchange with a Challenge, the client each Challenge with an Answer,
//! until the server sends Ready, with the key that cancels the session's
//! statements and the capabilities both have; or an Error, and closes.
//!
//! Then the client sends requests, and may send one before the last is
//! answered: each is answered in turn, in order.
//! * Query, a `<direct SQL statement>`: a Description, its Rows in
//!   batches and a Complete; or a Complete alone; or an Error.
//! * Prepare [ISO/IEC 9075:1992 §17.6]: Prepared, its parameters' types
//!   and its columns [§17.8]; or an Error.
//! * Execute [§17.10, §17.14]: a Complete; or, of a query, its cursor
//!   opened, a Description and Rows, as many as asked, then Suspended if it
//!   has more, which Fetch [§17.15] sends, or a Complete once it has none,
//!   its cursor then closed; or an Error.
//! * Close [§17.16] and Deallocate [§17.7]: a Complete, or an Error.
//!
//! A statement running is cancelled from another connection, whose first
//! and only message is Cancel with the key: the server closes it, and the
//! statement fails (SQLCODE -920).
//!
//! ```text
//! frame:      kind u8, length u32, then that many bytes of body
//! fields:     as `turborel::bytes`: integers little-endian, strings and
//!             bytes their length u32 then the bytes; lists their count
//!             u32 then each
//! value:      tag u8: 0 null; 1 character, a string; 2 exact, units i128
//!             then scale u32; 3 approximate, the f64's bits u64
//! data type:  tag u8: 1 character, length u32; 2 exact, precision u32
//!             then scale u32; 3 approximate, precision u32
//! parameter:  varying u8 (CHARACTER VARYING if 1); data type
//! column:     named u8, its name if 1; data type; not null u8
//! failure:    sqlcode i32 as u32; sqlstate, a string of 5; message;
//!             cited u8, the rule if 1; placed u8, the position u64 if 1
//! key:        16 bytes
//!
//! client      'H' hello: version u32, user, mechanism, initial bytes,
//!                 capabilities (strings)
//!             'A' answer: bytes
//!             'Q' query: the SQL
//!             'P' prepare: statement u32, the SQL
//!             'E' execute: statement u32, values, rows u32 (0: all)
//!             'F' fetch: statement u32, rows u32 (0: all)
//!             'C' close: statement u32
//!             'D' deallocate: statement u32
//!             'Z' cancel: key
//!             'X' terminate
//! server      'C' challenge: bytes
//!             'R' ready: authorization identifier, key, capabilities
//!             'P' prepared: statement u32, parameters, columns
//!             'D' description: columns
//!             'B' rows: each row's values
//!             'S' suspended
//!             'K' complete: sqlcode i32 as u32; sqlstate, a string of 5
//!             'E' error: failure
//! ```

mod fields;
mod reply;
mod request;

use turborel::binder::OutColumn;
use turborel::bytes::Put;
use turborel::db::Completion;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::value::DataType;

pub use fields::row_size;
pub use reply::Reply;
pub use request::Request;

/// The protocol's version, the first field of Hello.
pub const VERSION: u32 = 1;
/// \[DESIGN\] The TCP port a server listens on, and a client connects to,
/// unless told another.
pub const DEFAULT_PORT: u16 = 7422;
/// A frame's header: its kind and its body's length.
pub const HEADER: usize = 5;
/// \[DESIGN\] The default of the longest body either side reads: longer is
/// a connection failure, so a peer cannot make the other hold without
/// bound.
pub const MAX_BODY: u32 = 16 * 1024 * 1024;
/// The bytes of a cancel key: 128 bits, too many to guess.
pub const KEY_BYTES: usize = 16;

/// \[DESIGN\] What cancels a session's statements, from another
/// connection: a secret its Ready gives the client alone.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CancelKey(pub [u8; KEY_BYTES]);

impl std::fmt::Debug for CancelKey {
    /// Never the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CancelKey(..)")
    }
}

/// A column of a result, as described to a client.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Its name, if it has one.
    pub name: Option<String>,
    /// Its type.
    pub data_type: DataType,
    /// Whether it is constrained NOT NULL.
    pub not_null: bool,
}

impl From<&OutColumn> for Column {
    fn from(c: &OutColumn) -> Column {
        Column { name: c.name.clone(), data_type: c.data_type, not_null: c.not_null }
    }
}

/// A statement's completion as a client receives it: SQLCODE 0 or 100,
/// and its SQLSTATE, that of a warning [ISO/IEC 9075:1992 §22.1] among
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    /// Its SQLCODE.
    pub sqlcode: i32,
    /// Its SQLSTATE: five characters.
    pub sqlstate: String,
}

impl From<Completion> for Completed {
    fn from(c: Completion) -> Completed {
        Completed { sqlcode: c.sqlcode(), sqlstate: c.sqlstate().as_str().to_owned() }
    }
}

impl Completed {
    /// Whether it is a warning: SQLSTATE class 01.
    pub fn is_warning(&self) -> bool {
        self.sqlstate.starts_with("01")
    }
}

/// An error as a client receives it: everything the server's `SqlError`
/// says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// Its SQLCODE.
    pub sqlcode: i32,
    /// Its SQLSTATE: five characters.
    pub sqlstate: String,
    /// What happened, in plain words.
    pub message: String,
    /// The rule of a standard broken, if one is.
    pub rule: Option<String>,
    /// Where in the statement, in characters, if known.
    pub position: Option<u64>,
}

impl From<&SqlError> for Failure {
    fn from(e: &SqlError) -> Failure {
        Failure {
            sqlcode: e.sqlcode(),
            sqlstate: e.sqlstate().as_str().to_owned(),
            message: e.msg.clone(),
            rule: e.rule.clone(),
            position: e.pos.and_then(|p| u64::try_from(p).ok()),
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SQLCODE {} (SQLSTATE {}): {}", self.sqlcode, self.sqlstate, self.message)?;
        if let Some(rule) = &self.rule {
            write!(f, " [{rule}]")?;
        }
        if let Some(p) = self.position {
            write!(f, " (at character {p})")?;
        }
        Ok(())
    }
}

impl std::error::Error for Failure {}

impl Failure {
    /// Whether the connection it came on is lost: a connection exception
    /// (SQLSTATE class 08), or credentials refused (28), after which the
    /// server closes it.
    pub fn ends_connection(&self) -> bool {
        self.sqlstate.starts_with("08") || self.sqlstate.starts_with("28")
    }
}

/// A connection failure (SQLCODE -919, SQLSTATE 08006): `msg`.
pub fn broken(msg: impl Into<String>) -> SqlError {
    SqlError::new(Code::Connection, msg).with_state(SqlState::CONNECTION_FAILURE)
}

/// The kind and the body's length of the frame whose header is `header`.
///
/// # Errors
///
/// A connection failure if the body would be longer than `max`.
pub fn header(header: [u8; HEADER], max: u32) -> Result<(u8, usize)> {
    let [kind, length @ ..] = header;
    let length = u32::from_le_bytes(length);
    if length > max {
        return Err(broken(format!("[DESIGN] a message of {length} bytes is longer than the {max} taken")));
    }
    let length = usize::try_from(length).map_err(|_| broken("[DESIGN] a message is too long for this machine"))?;
    Ok((kind, length))
}

/// Appends a frame of kind `kind` whose body `body` writes.
pub(crate) fn frame(out: &mut Vec<u8>, kind: u8, body: impl FnOnce(&mut Vec<u8>)) {
    out.put_u8(kind);
    let at = out.len();
    out.put_u32(0);
    body(out);
    let length = out.len().saturating_sub(at).saturating_sub(HEADER.saturating_sub(1));
    let length = u32::try_from(length).unwrap_or(u32::MAX).to_le_bytes();
    if let Some(field) = out.get_mut(at..at.saturating_add(length.len())) {
        field.copy_from_slice(&length);
    }
}

// Unit tests: every message read back as written, and malformed ones
// refused (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
