//! What a server sends. Split from lib.rs (RULES.md R-SIZE-4).

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::params::ParameterType;
use turborel::storage::Row;

use crate::fields::{
    column, counted, failure, flag, parameter, put_column, put_count, put_failure, put_flag, put_parameter, put_row, put_strs, row, strs,
};
use crate::{CancelKey, Column, Completed, Failure, KEY_BYTES, broken, frame};

const CHALLENGE: u8 = b'C';
const READY: u8 = b'R';
const PREPARED: u8 = b'P';
const DESCRIPTION: u8 = b'D';
const ROWS: u8 = b'B';
const SUSPENDED: u8 = b'S';
const COMPLETE: u8 = b'K';
const ERROR: u8 = b'E';

/// What a server sends. Not comparable: its values compare as SQL
/// says, not as data; compare the bytes they encode to.
#[derive(Debug, Clone)]
pub enum Reply {
    /// The next step of an authentication exchange.
    Challenge(Vec<u8>),
    /// Authenticated: the session's authorization identifier, the key that
    /// cancels its statements, the capabilities the server has of those the
    /// client has, and the server's name and version.
    Ready {
        /// The session's authorization identifier.
        authorization: String,
        /// What cancels the session's statements, from another connection.
        key: CancelKey,
        /// The capabilities both have.
        capabilities: Vec<String>,
        /// The server's product, version and codename, as "turborel 0.0.0
        /// (alpha)" (`turborel::product`): told to
        /// an authenticated client alone.
        server: String,
    },
    /// [ISO/IEC 9075:1992 §17.8, §17.9 GR3, GR4] A statement prepared: its
    /// dynamic parameters' types, and its result's columns, of a query.
    Prepared {
        /// The statement's number.
        statement: u32,
        /// Each dynamic parameter's type, in order.
        parameters: Vec<ParameterType>,
        /// The columns of a query's rows; none of another statement.
        columns: Vec<Column>,
    },
    /// The columns of the rows that follow.
    Description(Vec<Column>),
    /// A batch of rows.
    Rows(Vec<Row>),
    /// The rows asked for were sent, and the cursor has more: fetch them.
    Suspended,
    /// A statement's completion: its SQLCODE, 0 or 100, and its SQLSTATE.
    Complete(Completed),
    /// A statement's failure, or the connection's.
    Error(Failure),
}

impl Reply {
    /// Appends this reply, framed, to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Reply::Challenge(bytes) => frame(out, CHALLENGE, |b| b.put_bytes(bytes)),
            Reply::Ready { authorization, key, capabilities, server } => frame(out, READY, |b| {
                b.put_str(authorization);
                b.extend_from_slice(&key.0);
                put_strs(b, capabilities);
                b.put_str(server);
            }),
            Reply::Prepared { statement, parameters, columns } => frame(out, PREPARED, |b| {
                b.put_u32(*statement);
                put_count(b, parameters.len());
                for p in parameters {
                    put_parameter(b, p);
                }
                put_columns(b, columns);
            }),
            Reply::Description(columns) => frame(out, DESCRIPTION, |b| put_columns(b, columns)),
            Reply::Rows(rows) => frame(out, ROWS, |b| {
                put_count(b, rows.len());
                for row in rows {
                    put_row(b, row);
                }
            }),
            Reply::Suspended => frame(out, SUSPENDED, |_| ()),
            Reply::Complete(c) => frame(out, COMPLETE, |b| {
                b.put_u32(c.sqlcode.cast_unsigned());
                b.put_str(&c.sqlstate);
                put_flag(b, c.active);
            }),
            Reply::Error(failure) => frame(out, ERROR, |b| put_failure(b, failure)),
        }
    }

    /// The reply of kind `kind` whose body is `body`.
    ///
    /// # Errors
    ///
    /// A connection failure if it is not a reply, or is malformed.
    pub fn decode(kind: u8, body: &[u8]) -> Result<Reply> {
        let mut r = Reader::received(body, "reply");
        let reply = match kind {
            CHALLENGE => Reply::Challenge(r.bytes()?.to_vec()),
            READY => {
                let authorization = r.str()?;
                let key = <[u8; KEY_BYTES]>::try_from(r.take(KEY_BYTES)?).map_err(|_| r.corrupt("a key is cut short"))?;
                Reply::Ready { authorization, key: CancelKey(key), capabilities: strs(&mut r)?, server: r.str()? }
            }
            PREPARED => Reply::Prepared { statement: r.u32()?, parameters: counted(&mut r, parameter)?, columns: counted(&mut r, column)? },
            DESCRIPTION => Reply::Description(counted(&mut r, column)?),
            ROWS => Reply::Rows(counted(&mut r, row)?),
            SUSPENDED => Reply::Suspended,
            COMPLETE => Reply::Complete(Completed { sqlcode: r.u32()?.cast_signed(), sqlstate: r.str()?, active: flag(&mut r)? }),
            ERROR => Reply::Error(failure(&mut r)?),
            other => return Err(broken(format!("[DESIGN] no reply is of kind {other}"))),
        };
        r.finish()?;
        Ok(reply)
    }
}

fn put_columns(b: &mut Vec<u8>, columns: &[Column]) {
    put_count(b, columns.len());
    for c in columns {
        put_column(b, c);
    }
}
