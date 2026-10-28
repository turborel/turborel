//! What a client sends. Split from lib.rs (RULES.md R-SIZE-4).

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::value::Value;

use crate::fields::{put_row, put_strs, row, strs};
use crate::{CancelKey, KEY_BYTES, broken, frame};

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

/// What a client sends.
#[derive(Debug, Clone)]
pub enum Request {
    /// The first message: the protocol version, the user, the mechanism of
    /// authentication and its first bytes, and the capabilities the client
    /// has (none is defined in version 1).
    Hello {
        /// `VERSION`, as the client speaks it.
        version: u32,
        /// Who the client says it is.
        user: String,
        /// The `Authenticator` to use, by its mechanism's name.
        mechanism: String,
        /// The mechanism's first bytes.
        initial: Vec<u8>,
        /// The capabilities the client has.
        capabilities: Vec<String>,
    },
    /// The client's answer to a Challenge.
    Answer(Vec<u8>),
    /// A `<direct SQL statement>` [ISO/IEC 9075:1992 §20.1]: all its rows
    /// sent.
    Query(String),
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
        /// A value for each of its dynamic parameters [§17.9 GR5].
        values: Vec<Value>,
        /// The most rows sent now, of a query: all if 0.
        rows: u32,
    },
    /// [ISO/IEC 9075:1992 §17.15] At most `rows` more rows of the statement's
    /// cursor, all if 0.
    Fetch {
        /// The statement's number.
        statement: u32,
        /// The most rows sent: all if 0.
        rows: u32,
    },
    /// [ISO/IEC 9075:1992 §17.16] Closes the statement's cursor.
    Close {
        /// The statement's number.
        statement: u32,
    },
    /// [ISO/IEC 9075:1992 §17.7] Destroys the prepared statement.
    Deallocate {
        /// The statement's number.
        statement: u32,
    },
    /// \[DESIGN\] The first and only message of a connection that cancels
    /// the statement another runs: the key its Ready gave.
    Cancel(CancelKey),
    /// The client is done; the server closes the connection.
    Terminate,
}

impl Request {
    /// Appends this request, framed, to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Request::Hello { version, user, mechanism, initial, capabilities } => frame(out, HELLO, |b| {
                b.put_u32(*version);
                b.put_str(user);
                b.put_str(mechanism);
                b.put_bytes(initial);
                put_strs(b, capabilities);
            }),
            Request::Answer(bytes) => frame(out, ANSWER, |b| b.put_bytes(bytes)),
            Request::Query(sql) => frame(out, QUERY, |b| b.put_str(sql)),
            Request::Prepare { statement, sql } => frame(out, PREPARE, |b| {
                b.put_u32(*statement);
                b.put_str(sql);
            }),
            Request::Execute { statement, values, rows } => frame(out, EXECUTE, |b| {
                b.put_u32(*statement);
                put_row(b, values);
                b.put_u32(*rows);
            }),
            Request::Fetch { statement, rows } => frame(out, FETCH, |b| {
                b.put_u32(*statement);
                b.put_u32(*rows);
            }),
            Request::Close { statement } => frame(out, CLOSE, |b| b.put_u32(*statement)),
            Request::Deallocate { statement } => frame(out, DEALLOCATE, |b| b.put_u32(*statement)),
            Request::Cancel(key) => frame(out, CANCEL, |b| b.extend_from_slice(&key.0)),
            Request::Terminate => frame(out, TERMINATE, |_| ()),
        }
    }

    /// The request of kind `kind` whose body is `body`.
    ///
    /// # Errors
    ///
    /// A connection failure if it is not a request, or is malformed.
    pub fn decode(kind: u8, body: &[u8]) -> Result<Request> {
        let mut r = Reader::received(body, "request");
        let request = match kind {
            HELLO => Request::Hello {
                version: r.u32()?,
                user: r.str()?,
                mechanism: r.str()?,
                initial: r.bytes()?.to_vec(),
                capabilities: strs(&mut r)?,
            },
            ANSWER => Request::Answer(r.bytes()?.to_vec()),
            QUERY => Request::Query(r.str()?),
            PREPARE => Request::Prepare { statement: r.u32()?, sql: r.str()? },
            EXECUTE => Request::Execute { statement: r.u32()?, values: row(&mut r)?, rows: r.u32()? },
            FETCH => Request::Fetch { statement: r.u32()?, rows: r.u32()? },
            CLOSE => Request::Close { statement: r.u32()? },
            DEALLOCATE => Request::Deallocate { statement: r.u32()? },
            CANCEL => {
                let key = <[u8; KEY_BYTES]>::try_from(r.take(KEY_BYTES)?).map_err(|_| r.corrupt("a key is cut short"))?;
                Request::Cancel(CancelKey(key))
            }
            TERMINATE => Request::Terminate,
            other => return Err(broken(format!("[DESIGN] no request is of kind {other}"))),
        };
        r.finish()?;
        Ok(request)
    }
}
