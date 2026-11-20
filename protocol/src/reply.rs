//! What a server sends: the replies of version 1's first form, and those
//! added since, each behind its capability (RULES.md D185).
//!
//! ```text
//! 'N' notice:     as `diagnostics.rs` says
//! 'T' status:     name, value (strings)
//! 'G' pong
//! ```

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::params::ParameterType;
use turborel::storage::Row;

use crate::declared::{declared, put_declared};
use crate::diagnostics::Notice;
use crate::fields::{
    column, counted, failure, flag, parameter, put_column, put_count, put_failure, put_flag, put_parameter, put_row, put_strs, row, strs,
};
use crate::tail::{CONDITIONS, GENERATED, MORE, POSITION, ROWS as INSERTED, TIME, Tail, put_long, put_mark};
use crate::{CancelKey, Column, Completed, Failure, KEY_BYTES, LobReply, Timing, broken, frame};

const CHALLENGE: u8 = b'C';
const READY: u8 = b'R';
const PREPARED: u8 = b'P';
const DESCRIPTION: u8 = b'D';
const ROWS: u8 = b'B';
const SUSPENDED: u8 = b'S';
const COMPLETE: u8 = b'K';
const ERROR: u8 = b'E';
const LOB: u8 = b'L';
const NOTICE: u8 = b'N';
const STATUS: u8 = b'T';
const PONG: u8 = b'G';

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
    /// \[DESIGN\] What a Lob request gave back (`lobs.rs`, RULES.md D79).
    Lob(LobReply),
    /// \[DESIGN\] With `notices`, a completion condition of class 01, or
    /// a notice of the server's own, before the statement's completion
    /// (RULES.md D187).
    Notice(Notice),
    /// \[DESIGN\] With `status`, a setting of the session, as it begins or
    /// as a statement changed it (RULES.md D190).
    Status {
        /// Its name.
        name: String,
        /// Its value, as SQL writes it; empty for a null.
        value: String,
    },
    /// \[DESIGN\] With `ping`, Ping's answer (RULES.md D189).
    Pong,
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
                put_long(b, POSITION, c.position);
                put_mark(b, MORE, c.more);
                put_long(b, INSERTED, c.rows);
                put_generated(b, &c.generated);
                put_conditions(b, &c.conditions);
                put_time(b, c.time);
            }),
            Reply::Error(failure) => frame(out, ERROR, |b| put_failure(b, failure)),
            Reply::Lob(lob) => frame(out, LOB, |b| lob.put(b)),
            Reply::Notice(notice) => frame(out, NOTICE, |b| notice.put(b)),
            Reply::Status { name, value } => frame(out, STATUS, |b| {
                b.put_str(name);
                b.put_str(value);
            }),
            Reply::Pong => frame(out, PONG, |_| ()),
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
            PREPARED => Reply::Prepared { statement: r.u32()?, parameters: counted(&mut r, parameter)?, columns: columns(&mut r)? },
            DESCRIPTION => Reply::Description(columns(&mut r)?),
            ROWS => Reply::Rows(counted(&mut r, row)?),
            SUSPENDED => Reply::Suspended,
            COMPLETE => Reply::Complete(completed(&mut r)?),
            ERROR => Reply::Error(failure(&mut r)?),
            LOB => Reply::Lob(LobReply::read(&mut r)?),
            NOTICE => Reply::Notice(Notice::read(&mut r)?),
            STATUS => Reply::Status { name: r.str()?, value: r.str()? },
            PONG => Reply::Pong,
            other => return Err(broken(format!("[DESIGN] no reply is of kind {other}"))),
        };
        r.finish()?;
        Ok(reply)
    }

    /// \[DESIGN\] RULES.md D393: whether a reply of kind `kind` and body
    /// `body`, decoded or not, is the last its request is answered with,
    /// by the order of a statement's replies. A Description, Rows, a
    /// Notice or a Status has more after it; a Complete has if its tail
    /// says more (tag 5), read as `completed_head` reads it; any other
    /// reply ends its request's. A Describe of a cursor, answered by a
    /// Description alone, is its reader's to know.
    pub fn ends_request(kind: u8, body: &[u8]) -> bool {
        match kind {
            DESCRIPTION | ROWS | NOTICE | STATUS => false,
            COMPLETE => Reply::completed_head(kind, body).is_none_or(|c| !c.more),
            _ => true,
        }
    }

    /// \[DESIGN\] RULES.md D393: the head of a reply of kind `kind` and
    /// body `body`, if it is a Complete whose head reads. That is its
    /// SQLCODE, SQLSTATE and whether a transaction is active, and of its
    /// tail the commit's position and whether more replies follow, which
    /// come before any field a client may fail to take, a condition (tag
    /// 17).
    /// So a client that cannot take the rest still knows where its
    /// request's replies end and the session's state.
    pub fn completed_head(kind: u8, body: &[u8]) -> Option<Completed> {
        if kind != COMPLETE {
            return None;
        }
        let mut r = Reader::received(body, "reply");
        let (sqlcode, sqlstate, active) = (r.u32().ok()?.cast_signed(), r.str().ok()?, flag(&mut r).ok()?);
        let mut c = Completed { sqlcode, sqlstate, active, ..Completed::default() };
        let mut tail = Tail::new();
        while let Ok(Some(tag)) = tail.next(&mut r, &[POSITION, MORE, INSERTED, GENERATED, CONDITIONS, TIME]) {
            match tag {
                POSITION => c.position = Some(r.u64().ok()?),
                MORE => c.more = true,
                _ => break,
            }
        }
        Some(c)
    }
}

/// Columns, then the tail of their declared types, if any has one.
fn put_columns(b: &mut Vec<u8>, columns: &[Column]) {
    put_count(b, columns.len());
    for c in columns {
        put_column(b, c);
    }
    put_declared(b, columns);
}

/// The columns `put_columns` wrote, each with its declared type if told.
fn columns(r: &mut Reader<'_>) -> Result<Vec<Column>> {
    let mut all = counted(r, column)?;
    declared(r, &mut all)?;
    Ok(all)
}

/// A Complete's body, its tail of a commit's position, more replies to
/// come, and the rows inserted.
fn completed(r: &mut Reader<'_>) -> Result<Completed> {
    let (sqlcode, sqlstate, active) = (r.u32()?.cast_signed(), r.str()?, flag(r)?);
    let mut c = Completed { sqlcode, sqlstate, active, ..Completed::default() };
    let mut tail = Tail::new();
    while let Some(tag) = tail.next(r, &[POSITION, MORE, INSERTED, GENERATED, CONDITIONS, TIME])? {
        match tag {
            POSITION => c.position = Some(r.u64()?),
            MORE => c.more = true,
            INSERTED => c.rows = Some(r.u64()?),
            GENERATED => c.generated = generated(r)?,
            CONDITIONS => c.conditions = conditions(r)?,
            _ => c.time = Some(Timing { total: r.u64()?, queued: r.u64()?, locks: r.u64()? }),
        }
    }
    Ok(c)
}

/// Writes the server's time for a statement, if told.
fn put_time(b: &mut Vec<u8>, time: Option<Timing>) {
    let Some(t) = time else { return };
    b.put_u8(TIME);
    for n in [t.total, t.queued, t.locks] {
        b.put_u64(n);
    }
}

/// Writes the identity values an INSERT generated, if any.
fn put_generated(b: &mut Vec<u8>, generated: &[String]) {
    if generated.is_empty() {
        return;
    }
    b.put_u8(GENERATED);
    put_strs(b, generated);
}

/// The identity values `put_generated` wrote.
fn generated(r: &mut Reader<'_>) -> Result<Vec<String>> {
    strs(r)
}

/// Writes the conditions a statement raised, if any: each the body of an
/// Error, so its own tail ends where it does.
fn put_conditions(b: &mut Vec<u8>, conditions: &[Failure]) {
    if conditions.is_empty() {
        return;
    }
    b.put_u8(CONDITIONS);
    put_count(b, conditions.len());
    for condition in conditions {
        let mut body = Vec::new();
        put_failure(&mut body, condition);
        b.put_bytes(&body);
    }
}

/// The conditions `put_conditions` wrote.
fn conditions(r: &mut Reader<'_>) -> Result<Vec<Failure>> {
    counted(r, |r| {
        let mut inner = Reader::new(r.bytes()?, "a condition");
        let condition = failure(&mut inner)?;
        inner.finish()?;
        Ok(condition)
    })
}
