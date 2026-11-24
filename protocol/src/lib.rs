//! \[DESIGN\] The native wire protocol, version 1 (RULES.md D42, D45): its
//! messages and their encoding, without I/O, so the server and the client
//! share one codec and tests can check it byte by byte.
//!
//! A connection begins with the client's Hello: the protocol version, the
//! user, a mechanism of authentication with its first bytes, the
//! capabilities the client asks for (`capability.rs`) and the connection's
//! attributes. The server answers each step of the
//! exchange with a Challenge, the client each Challenge with an Answer,
//! until the server sends Ready, with the session's authorization
//! identifier, the key that cancels its statements, the capabilities both
//! have and the server's product; or an Error, and closes. The steps of
//! SCRAM-SHA-256 and SCRAM-SHA-256-PLUS, both sides', are `scram`, with the
//! feature `scram`.
//!
//! Then the client sends requests, and may send one before the last is
//! answered: each is answered in turn, in order.
//! * Query, a `<direct SQL statement>`: a Description, its Rows in
//!   batches and a Complete; or a Complete alone; or an Error.
//! * Prepare [ISO/IEC 9075:1992 §17.6]: Prepared, its parameters' types
//!   and its columns [§17.8]; or an Error.
//! * Execute [§17.10, §17.14]: a Complete; or, of a query, its cursor
//!   opened, its Rows, of the columns Prepared gave, as many as asked,
//!   then Suspended if it has more, which Fetch [§17.15] sends, or a
//!   Complete once it has none, its cursor then closed; or an Error.
//! * Close [§17.16] and Deallocate [§17.7]: a Complete, or an Error.
//!
//! A Query or an Execute may say the transaction ends with its statement
//! \[EXTENSION\]: it is committed once the statement succeeds, or rolled
//! back if it fails, and the Complete or the Error says no transaction is
//! active, so a statement that is its own transaction, as a driver's
//! autocommit has it, is one request and one reply. A query's cursor left
//! open keeps the transaction.
//!
//! Version 1 grew after its first form by capabilities (RULES.md D185):
//! each added behaviour is granted in Ready only if the client asked for
//! it in Hello, so a peer that never asks sees the first form alone. The
//! fields a message of the first form gained are its tail (`tail.rs`),
//! written only where granted; the messages added are in `request/added.rs`
//! and `reply.rs`; compressed frames are `compress.rs`.
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
//!             then scale u32; 3 approximate, the f64's bits u64; 4 of a
//!             type beyond the standard (RULES.md D53), its type's name, its
//!             text, then its bytes; 6 a binary string, its octets as
//!             bytes; 8 date, 9 time, 11 timestamp, each zoned u8,
//!             precision u8, the zone's displacement in minutes, an i16's
//!             bits in a u32, then the instant, microseconds in UTC, i64 as
//!             u64; 13 interval, its qualifier
//!             as its data type's, then units i128; 14 truth value, 0 or
//!             1 u8; 15 decimal floating point, coefficient i128 then
//!             exponent, an i32's bits in a u32; 16 to 26 the constructed,
//!             large object, JSON and XML values, as `fields.rs` writes
//!             them, 26 XML its serialization as CONTENT, a string
//! data type:  tag u8: 1 character, 5 character varying, 6 binary, 7
//!             binary varying, each length u32; 2 exact, precision u32 then scale
//!             u32; 3 approximate, precision u32; 4 beyond the standard, its
//!             name, then its parameters, a list of u32; 8 date, 9 time,
//!             10 time with time zone, 11 timestamp, 12 timestamp with time
//!             zone, each precision u32; 13 interval, start field, end field,
//!             leading precision and fractional precision, each u32; 14
//!             boolean; 15 decfloat, precision u32; 16 to 25 as `fields.rs`
//!             writes them; 26 XML, its primary
//!             modifier u8 (0 DOCUMENT, 1 CONTENT, 2 SEQUENCE), then its
//!             secondary u8 (0 UNTYPED or none, 1 ANY)
//! parameter:  varying u8 (CHARACTER VARYING if 1); data type
//! column:     named u8, its name if 1; data type; not null u8
//! failure:    sqlcode i32 as u32; sqlstate, a string of 5; message;
//!             cited u8, the rule if 1; placed u8, the position u64 if 1;
//!             active u8 (the transaction active after it if 1); tail:
//!             its diagnostics, with `diagnostics`
//! key:        16 bytes
//!
//! client      'H' hello: version u32, user, mechanism, initial bytes,
//!                 capabilities (strings); tail: attributes
//!             'A' answer: bytes
//!             'Q' query: the SQL, ends u8 (the transaction ends with it if
//!                 1); tail: attributes, multi
//!             'P' prepare: statement u32, the SQL
//!             'E' execute: statement u32, values, rows u32 (0: all), ends
//!                 u8; tail: attributes, cursor
//!             'F' fetch: statement u32, rows u32 (0: all); tail: cursor
//!             'C' close: statement u32; tail: cursor
//!             'D' deallocate: statement u32
//!             'Z' cancel: key
//!             'X' terminate
//! server      'C' challenge: bytes
//!             'R' ready: authorization identifier, key, capabilities, the
//!                 server's product and version
//!             'P' prepared: statement u32, parameters, columns; tail:
//!                 declared
//!             'D' description: columns; tail: declared
//!             'B' rows: each row's values
//!             'S' suspended
//!             'K' complete: sqlcode i32 as u32; sqlstate, a string of 5;
//!                 active u8, as a failure's; tail: position, more, rows,
//!                 generated, conditions, time
//!             'E' error: failure
//! ```
//!
//! The messages version 1 added, each behind its capability, and every
//! layout byte for byte, are in the protocol's reference
//! (`doc/client/05-protocol.md`).

pub mod capability;
pub mod compress;
mod debug;
mod declared;
pub mod der;
mod diagnostics;
mod fields;
mod lobs;
#[cfg(feature = "scram")]
pub mod packages;
mod reply;
mod request;
#[cfg(feature = "scram")]
pub mod scram;
pub mod split;
mod tail;
pub mod zlib;

use turborel::binder::OutColumn;
use turborel::bytes::Put;
use turborel::db::Completion;
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::value::DataType;

pub use capability::Capabilities;
pub use debug::{DebugBreakpoint, DebugReply, DebugRequest};
pub use declared::Declared;
pub use diagnostics::{Diagnostics, ERROR, FATAL, NOTICE, Notice, WARNING};
pub use fields::row_size;
pub use lobs::{LobReply, LobRequest};
pub use reply::Reply;
pub use request::{Described, Request};
pub use tail::{MAX_ATTRIBUTE_NAME, MAX_ATTRIBUTE_VALUE, MAX_ATTRIBUTES};

/// \[DESIGN\] Attributes of a connection or a request: each a name and a
/// value (RULES.md D191).
pub type Attributes = Vec<(String, String)>;

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
    /// \[DESIGN\] With `declared-types`, the type it was declared with, as
    /// INTEGER of an exact numeric type (RULES.md D396). Its tail.
    pub declared: Option<Declared>,
}

impl From<&OutColumn> for Column {
    /// Without its declared type, as a client not granted
    /// `declared-types` is told.
    fn from(c: &OutColumn) -> Column {
        Column { name: c.name.clone(), data_type: c.data_type.clone(), not_null: c.not_null, declared: None }
    }
}

impl Column {
    /// \[DESIGN\] RULES.md D396: it, with its declared type.
    #[must_use]
    pub fn with_declared(self) -> Column {
        let declared = Some(Declared::of(&self.data_type));
        Column { declared, ..self }
    }

    /// Its type as SQL writes it, as it was declared if told, else as its
    /// tag and parameters say, so INTEGER is NUMERIC(10,0).
    pub fn type_written(&self) -> String {
        self.declared.as_ref().map_or_else(|| self.data_type.to_string(), |d| d.written.clone())
    }
}

/// A statement's completion as a client receives it: SQLCODE 0 or 100,
/// and its SQLSTATE, that of a warning [ISO/IEC 9075:1992 §22.1] among
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Completed {
    /// Its SQLCODE.
    pub sqlcode: i32,
    /// Its SQLSTATE: five characters.
    pub sqlstate: String,
    /// Whether the session's transaction is active after it [ISO/IEC
    /// 9075:1992 §4.28], as a client's connection statements need to know
    /// [§15.1 GR1, §15.3 GR6].
    pub active: bool,
    /// \[DESIGN\] With `commit-position`, the log position of the commit it
    /// made, if it committed (RULES.md D194), which a replica's session
    /// waits for with REPLICATION.READ_AFTER. Its tail.
    pub position: Option<u64>,
    /// \[DESIGN\] With `multi-statement`, whether another statement's
    /// replies follow, of a Query of several (RULES.md D192). Its tail.
    pub more: bool,
    /// \[DESIGN\] With `row-counts`, the rows its statement inserted,
    /// updated, deleted or merged, as GET DIAGNOSTICS ROW_COUNT gives
    /// them, or the rows a query returned (RULES.md D197); with `copy`,
    /// the rows a copy inserted (D195). Its tail.
    pub rows: Option<u64>,
    /// \[DESIGN\] With `generated-values`, the identity values an INSERT
    /// generated, in the order of its rows, as SQL writes them (RULES.md
    /// D198). Its tail.
    pub generated: Vec<String>,
    /// \[DESIGN\] With `conditions`, each condition its statement raised,
    /// in order (RULES.md D187). Its tail.
    pub conditions: Vec<Failure>,
    /// \[DESIGN\] With `row-counts`, the server's time for its statement
    /// (RULES.md D240). Its tail.
    pub time: Option<Timing>,
}

/// \[DESIGN\] The server's time for a statement, in microseconds (RULES.md
/// D240): from when a worker began its request until its completion was
/// ready, a cursor's summed over the Execute and every Fetch, a copy's
/// over its requests; the time its requests waited for a worker before
/// that; and the time it waited for locks, a part of the whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    /// The whole time, from when a worker began each request.
    pub total: u64,
    /// The time its requests waited, read, for a worker to run them,
    /// before the whole and not in it.
    pub queued: u64,
    /// The time it waited for locks other transactions held, in the whole.
    pub locks: u64,
}

impl Timing {
    /// This and `other`, summed.
    #[must_use]
    pub fn plus(self, other: Timing) -> Timing {
        Timing {
            total: self.total.saturating_add(other.total),
            queued: self.queued.saturating_add(other.queued),
            locks: self.locks.saturating_add(other.locks),
        }
    }
}

impl From<Completion> for Completed {
    fn from(c: Completion) -> Completed {
        Completed::of(c.sqlcode(), c.sqlstate().as_str())
    }
}

impl Completed {
    /// A completion of `sqlcode` and `sqlstate`, no transaction active
    /// after it, its tail empty.
    pub fn of(sqlcode: i32, sqlstate: &str) -> Completed {
        Completed { sqlcode, sqlstate: sqlstate.to_owned(), ..Completed::default() }
    }

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
    /// Whether the session's transaction is active after it, as
    /// `Completed::active`.
    pub active: bool,
    /// \[DESIGN\] With `diagnostics`, its severity and what it is about
    /// (RULES.md D186). Its tail.
    pub diagnostics: Option<Box<Diagnostics>>,
}

impl From<&SqlError> for Failure {
    fn from(e: &SqlError) -> Failure {
        let mut failure = Failure {
            sqlcode: e.sqlcode(),
            sqlstate: e.sqlstate().as_str().to_owned(),
            message: e.msg.clone(),
            rule: e.rule.clone(),
            position: e.pos.and_then(|p| u64::try_from(p).ok()),
            active: false,
            diagnostics: None,
        };
        let severity = if failure.ends_connection() { FATAL } else { ERROR };
        failure.diagnostics = Some(Box::new(Diagnostics::of(e, severity)));
        failure
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
    /// It, without its diagnostics: as a peer not granted `diagnostics`
    /// reads it.
    #[must_use]
    pub fn plain(self) -> Failure {
        Failure { diagnostics: None, ..self }
    }

    /// Whether the connection it came on is lost: a connection exception
    /// (SQLSTATE class 08), or credentials refused as it is made (28, of a
    /// connection exception's SQLCODE), after which the server closes it.
    /// A statement's 28000 [ISO/IEC 9075:1992 §16.4 GR3, GR5] leaves it;
    /// so does 08003, connection does not exist, which names another
    /// connection, as END CONNECTION of one not served does.
    pub fn ends_connection(&self) -> bool {
        let connecting = self.sqlcode == SqlError::new(Code::Connection, "").sqlcode();
        let another = self.sqlstate == SqlState::CONNECTION_DOES_NOT_EXIST.as_str();
        (self.sqlstate.starts_with("08") && !another) || (connecting && self.sqlstate.starts_with("28"))
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
