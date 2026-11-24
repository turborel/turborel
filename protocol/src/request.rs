//! What a client sends: the messages of version 1's first form, and those
//! added since, each behind its capability (`added.rs`).

mod added;

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::storage::Row;
use turborel::value::Value;
use turborel::value::types::TypeRegistry;

use crate::fields::{flag, put_flag, put_row, put_strs, row, strs};
use crate::tail::{ATTRIBUTES, CURSOR, MULTI, Tail, attributes, put_attributes, put_mark, put_number};
use crate::{Attributes, CancelKey, DebugRequest, KEY_BYTES, LobRequest, broken, frame};

const HELLO: u8 = b'H';
const ANSWER: u8 = b'A';
const QUERY: u8 = b'Q';
const PREPARE: u8 = b'P';
const EXECUTE: u8 = b'E';
const FETCH: u8 = b'F';
const CLOSE: u8 = b'C';
const DEALLOCATE: u8 = b'D';
const CANCEL: u8 = b'Z';
const TERMINATE: u8 = b'X';
const LOB: u8 = b'L';

/// What Describe describes (RULES.md D188).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Described {
    /// A prepared statement, by its number: its parameters and columns.
    Statement(u32),
    /// An open cursor of a prepared statement: its columns.
    Cursor {
        /// The statement's number.
        statement: u32,
        /// The cursor's number, the client's to choose in Execute.
        cursor: u32,
    },
}

/// What a client sends.
#[derive(Debug, Clone)]
pub enum Request {
    /// The first message: the protocol version, the user, the mechanism of
    /// authentication and its first bytes, the capabilities the client
    /// asks for (`capability.rs`), and the connection's attributes.
    Hello {
        /// `VERSION`, as the client speaks it.
        version: u32,
        /// Who the client says it is.
        user: String,
        /// The `Authenticator` to use, by its mechanism's name.
        mechanism: String,
        /// The mechanism's first bytes.
        initial: Vec<u8>,
        /// The capabilities the client asks for.
        capabilities: Vec<String>,
        /// \[DESIGN\] The connection's attributes, each a name and a value,
        /// at most `MAX_ATTRIBUTES` (RULES.md D191): shown in
        /// INFORMATION_SCHEMA.CONNECTION_ATTRIBUTES. Its tail.
        attributes: Attributes,
    },
    /// The client's answer to a Challenge.
    Answer(Vec<u8>),
    /// A `<direct SQL statement>` [ISO/IEC 9075:1992 §20.1]: all its rows
    /// sent.
    Query {
        /// The statement.
        sql: String,
        /// \[EXTENSION\] Whether the transaction ends with the statement:
        /// committed once it succeeds, rolled back if it fails, as a
        /// COMMIT or a ROLLBACK sent after it would, so a statement that
        /// is its own transaction is one request and one reply (RULES.md
        /// D45). Nothing ends if no transaction is active after it.
        ends: bool,
        /// \[DESIGN\] The request's attributes (RULES.md D191), with
        /// `query-attributes`: read by QUERY_ATTRIBUTE. Its tail.
        attributes: Attributes,
        /// \[DESIGN\] Whether `sql` is several statements, run one by one
        /// (RULES.md D192), with `multi-statement`. Its tail.
        multi: bool,
    },
    /// [ISO/IEC 9075:1992 §17.6] Prepares `sql` as the client's statement
    /// `statement`, in place of one it had of that number \[GR6\].
    Prepare {
        /// The statement's number, the client's to choose.
        statement: u32,
        /// The `<preparable statement>`.
        sql: String,
    },
    /// [ISO/IEC 9075:1992 §17.10, §17.14] Executes the prepared statement,
    /// or a query opens its cursor and sends at most `rows` of its rows,
    /// all if 0.
    Execute {
        /// The statement's number.
        statement: u32,
        /// \[DESIGN\] The number of the cursor it opens, of a query, with
        /// `cursors` (RULES.md D188); 0 otherwise. Its tail.
        cursor: u32,
        /// A value for each of its dynamic parameters [§17.9 GR5].
        values: Vec<Value>,
        /// The most rows sent now, of a query: all if 0.
        rows: u32,
        /// \[EXTENSION\] Whether the transaction ends with the statement,
        /// as a Query's; a query's cursor left open, its rows not all
        /// sent, keeps it.
        ends: bool,
        /// \[DESIGN\] The request's attributes, as a Query's. Its tail.
        attributes: Attributes,
    },
    /// [ISO/IEC 9075:1992 §17.15] At most `rows` more rows of the statement's
    /// cursor, all if 0.
    Fetch {
        /// The statement's number.
        statement: u32,
        /// The cursor's number, as Execute gave it. Its tail.
        cursor: u32,
        /// The most rows sent: all if 0.
        rows: u32,
    },
    /// [ISO/IEC 9075:1992 §17.16] Closes the statement's cursor.
    Close {
        /// The statement's number.
        statement: u32,
        /// The cursor's number, as Execute gave it. Its tail.
        cursor: u32,
    },
    /// [ISO/IEC 9075:1992 §17.7] Destroys the prepared statement.
    Deallocate {
        /// The statement's number.
        statement: u32,
    },
    /// \[DESIGN\] What the client asks of a large object it reaches by its
    /// locator (`lobs.rs`, RULES.md D79).
    Lob(LobRequest),
    /// \[DESIGN\] The first and only message of a connection that cancels
    /// the statement another runs: the key its Ready gave.
    Cancel(CancelKey),
    /// The client is done; the server closes the connection.
    Terminate,
    /// \[DESIGN\] With `describe`, a prepared statement's parameters and
    /// columns, as Prepared gave them, or an open cursor's columns
    /// (RULES.md D188).
    Describe(Described),
    /// \[DESIGN\] With `ping`, a Pong, from the connection's task alone
    /// (RULES.md D189).
    Ping,
    /// \[DESIGN\] With `reset`, the session as it was at login, its
    /// authentication and cancel key kept (RULES.md D189).
    Reset,
    /// \[DESIGN\] With `change-user`, the connection authenticated again as
    /// `user`, as Hello does, then given a new session and a new cancel key
    /// (RULES.md D189).
    ChangeUser {
        /// Who the client says it is now.
        user: String,
        /// The `Authenticator` to use, by its mechanism's name.
        mechanism: String,
        /// The mechanism's first bytes.
        initial: Vec<u8>,
    },
    /// \[DESIGN\] With `copy`, rows into the columns `columns` of the table
    /// `table`, sent in CopyData until CopyEnd, inserted as they come, with
    /// no reply until CopyEnd's (RULES.md D195).
    CopyIn {
        /// The table, as SQL names it.
        table: String,
        /// Its columns the rows give values of, in order; all, in order,
        /// if none.
        columns: Vec<String>,
        /// Whether the transaction ends with the copy, as a Query's.
        ends: bool,
    },
    /// \[DESIGN\] Rows of the copy in progress.
    CopyData(Vec<Row>),
    /// \[DESIGN\] The copy's end: a Complete of the rows it inserted, or an
    /// Error; every row undone if `abandon`.
    CopyEnd {
        /// Whether the client gives the copy up.
        abandon: bool,
    },
    /// \[EXTENSION\] With `debugger`, what a debugger asks of the session
    /// it debugs (`debug.rs`, RULES.md D1030).
    Debug(DebugRequest),
}

impl Request {
    /// Appends this request, framed, to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Request::Hello { version, user, mechanism, initial, capabilities, attributes } => frame(out, HELLO, |b| {
                b.put_u32(*version);
                b.put_str(user);
                b.put_str(mechanism);
                b.put_bytes(initial);
                put_strs(b, capabilities);
                put_attributes(b, ATTRIBUTES, attributes);
            }),
            Request::Answer(bytes) => frame(out, ANSWER, |b| b.put_bytes(bytes)),
            Request::Query { sql, ends, attributes, multi } => frame(out, QUERY, |b| {
                b.put_str(sql);
                put_flag(b, *ends);
                put_attributes(b, ATTRIBUTES, attributes);
                put_mark(b, MULTI, *multi);
            }),
            Request::Prepare { statement, sql } => frame(out, PREPARE, |b| {
                b.put_u32(*statement);
                b.put_str(sql);
            }),
            Request::Execute { statement, cursor, values, rows, ends, attributes } => frame(out, EXECUTE, |b| {
                b.put_u32(*statement);
                put_row(b, values);
                b.put_u32(*rows);
                put_flag(b, *ends);
                put_attributes(b, ATTRIBUTES, attributes);
                put_number(b, CURSOR, *cursor, 0);
            }),
            Request::Fetch { statement, cursor, rows } => frame(out, FETCH, |b| {
                b.put_u32(*statement);
                b.put_u32(*rows);
                put_number(b, CURSOR, *cursor, 0);
            }),
            Request::Close { statement, cursor } => frame(out, CLOSE, |b| {
                b.put_u32(*statement);
                put_number(b, CURSOR, *cursor, 0);
            }),
            Request::Deallocate { statement } => frame(out, DEALLOCATE, |b| b.put_u32(*statement)),
            Request::Cancel(key) => frame(out, CANCEL, |b| b.extend_from_slice(&key.0)),
            Request::Terminate => frame(out, TERMINATE, |_| ()),
            Request::Lob(lob) => frame(out, LOB, |b| lob.put(b)),
            Request::Describe(_)
            | Request::Ping
            | Request::Reset
            | Request::ChangeUser { .. }
            | Request::CopyIn { .. }
            | Request::CopyData(_)
            | Request::CopyEnd { .. }
            | Request::Debug(_) => self.encode_added(out),
        }
    }

    /// The request of kind `kind` whose body is `body`.
    ///
    /// # Errors
    ///
    /// A connection failure if it is not a request, or is malformed.
    pub fn decode(kind: u8, body: &[u8]) -> Result<Request> {
        Request::decode_with(kind, body, &TypeRegistry::default())
    }

    /// The request of kind `kind` whose body is `body`, its values of types
    /// beyond the standard found among `types` (RULES.md D53).
    ///
    /// # Errors
    ///
    /// A connection failure if it is not a request, or is malformed.
    pub fn decode_with(kind: u8, body: &[u8], types: &TypeRegistry) -> Result<Request> {
        let mut r = Reader::received(body, "request").with_types(types);
        let request = match kind {
            HELLO => hello(&mut r)?,
            ANSWER => Request::Answer(r.bytes()?.to_vec()),
            QUERY => {
                let (sql, ends) = (r.str()?, flag(&mut r)?);
                let (attributes, multi, _) = tail(&mut r, &[ATTRIBUTES, MULTI])?;
                Request::Query { sql, ends, attributes, multi }
            }
            PREPARE => Request::Prepare { statement: r.u32()?, sql: r.str()? },
            EXECUTE => {
                let (statement, values, rows, ends) = (r.u32()?, row(&mut r)?, r.u32()?, flag(&mut r)?);
                let (attributes, _, cursor) = tail(&mut r, &[ATTRIBUTES, CURSOR])?;
                Request::Execute { statement, cursor, values, rows, ends, attributes }
            }
            FETCH => {
                let (statement, rows) = (r.u32()?, r.u32()?);
                Request::Fetch { statement, cursor: tail(&mut r, &[CURSOR])?.2, rows }
            }
            CLOSE => Request::Close { statement: r.u32()?, cursor: tail(&mut r, &[CURSOR])?.2 },
            DEALLOCATE => Request::Deallocate { statement: r.u32()? },
            CANCEL => {
                let key = <[u8; KEY_BYTES]>::try_from(r.take(KEY_BYTES)?).map_err(|_| r.corrupt("a key is cut short"))?;
                Request::Cancel(CancelKey(key))
            }
            TERMINATE => Request::Terminate,
            LOB => Request::Lob(LobRequest::read(&mut r)?),
            other => added::decode(other, &mut r)?.ok_or_else(|| broken(format!("[DESIGN] no request is of kind {other}")))?,
        };
        r.finish()?;
        Ok(request)
    }
}

/// A Hello, its attributes in its tail.
fn hello(r: &mut Reader<'_>) -> Result<Request> {
    let (version, user, mechanism, initial, capabilities) = (r.u32()?, r.str()?, r.str()?, r.bytes()?.to_vec(), strs(r)?);
    let (attributes, _, _) = tail(r, &[ATTRIBUTES])?;
    Ok(Request::Hello { version, user, mechanism, initial, capabilities, attributes })
}

/// A request's tail, of the tags `taken`: its attributes, whether it is
/// several statements, and its cursor, each its default if absent.
fn tail(r: &mut Reader<'_>, taken: &[u8]) -> Result<(Attributes, bool, u32)> {
    let (mut found, mut tail) = ((Vec::new(), false, 0), Tail::new());
    while let Some(tag) = tail.next(r, taken)? {
        match tag {
            ATTRIBUTES => found.0 = attributes(r)?,
            MULTI => found.1 = true,
            _ => found.2 = r.u32()?,
        }
    }
    Ok(found)
}
