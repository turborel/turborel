//! \[DESIGN\] The frames and messages of the plug-in protocol, version 1
//! (RULES.md D167), as `doc/client/04-plugins.md` specifies them for
//! programs in any language. A program written in Rust may use this
//! module as it is.
//!
//! ```text
//! frame:  length u32 (of what follows) | type u8 | body
//! str:    length u32 | UTF-8 bytes;  bytes: length u32 | octets
//! HELLO    1  protocol u32 | extension str | capabilities (count u32, str each)
//!             | max_message_bytes u64 | max_call_bytes u64 | memory_bytes u64
//! WELCOME  2  protocol u32 | name str | version str | capabilities | kinds (count u32, u8 each)
//! CALL     3  call u64 | kind u8 | op u8 | object str | payload (the rest)
//! RESULT   4  call u64 | payload (the rest)
//! ERROR    5  call u64 | sqlstate (5 bytes) | message str
//! CANCEL   6  call u64
//! SHUTDOWN 7  (nothing)
//! SQL      8  call u64 | mode u8 (0 run, 1 rows) | text str | values
//! SQLDONE  9  call u64 | rows
//! SETTINGS 10 names (count u32, str each) | values: the settings' values
//!             the next CALL runs with, sent before it when they changed
//!             since the last the process was sent (RULES.md D282)
//! ```
//!
//! Every integer is little-endian, as `bytes.rs` writes every other
//! format.

use std::io::{ErrorKind, Read, Write};

use super::errors::violation;
use super::payload::{put_strings, strings};
use super::values::{put_values, values};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::Value;

/// The type byte of HELLO.
pub const HELLO: u8 = 1;
/// The type byte of WELCOME.
pub const WELCOME: u8 = 2;
/// The type byte of CALL.
pub const CALL: u8 = 3;
/// The type byte of RESULT.
pub const RESULT: u8 = 4;
/// The type byte of ERROR.
pub const ERROR: u8 = 5;
/// The type byte of CANCEL.
pub const CANCEL: u8 = 6;
/// The type byte of SHUTDOWN.
pub const SHUTDOWN: u8 = 7;
/// The type byte of SQL.
pub const SQL: u8 = 8;
/// The type byte of SQLDONE.
pub const SQL_DONE: u8 = 9;
/// The type byte of SETTINGS.
pub const SETTINGS: u8 = 10;

/// The bytes of a frame's length.
const LENGTH_BYTES: usize = 4;
/// The bytes of an SQLSTATE.
const SQLSTATE_BYTES: usize = 5;

/// The capability of rows batched in one CALL.
pub const BATCH: &str = "batch";
/// The capability of SQL a procedure or table function runs through the
/// host (SQL and SQLDONE).
pub const SQL_CAPABILITY: &str = "sql";
/// The capability of CANCEL read while a call runs.
pub const CANCEL_CAPABILITY: &str = "cancel";
/// The capability of SETTINGS, the values of the settings a manifest
/// declares and reads, sent before a CALL as they change (RULES.md D282).
pub const SETTINGS_CAPABILITY: &str = "settings";
/// The capabilities a host of protocol 1 offers.
pub const HOST_CAPABILITIES: [&str; 4] = [BATCH, SQL_CAPABILITY, CANCEL_CAPABILITY, SETTINGS_CAPABILITY];

/// What the host tells a program first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The protocol's version: 1.
    pub protocol: u32,
    /// The extension's name.
    pub extension: String,
    /// What the host offers.
    pub capabilities: Vec<String>,
    /// The most bytes of one frame either side sends.
    pub max_message_bytes: u64,
    /// The most bytes the program sends in one call, all frames together.
    pub max_call_bytes: u64,
    /// The memory the program is to stay within.
    pub memory_bytes: u64,
}

/// What a program answers HELLO with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    /// The protocol's version: 1.
    pub protocol: u32,
    /// The extension's name, as its manifest gives it.
    pub name: String,
    /// The program's version.
    pub version: String,
    /// What it takes of what the host offered.
    pub capabilities: Vec<String>,
    /// The codes of the kinds it serves.
    pub kinds: Vec<u8>,
}

/// A call of an object of the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// Its number, which its answers repeat.
    pub call: u64,
    /// The code of the object's kind.
    pub kind: u8,
    /// The operation of the kind.
    pub op: u8,
    /// The object's name.
    pub object: String,
    /// What the operation carries (`payload.rs`).
    pub payload: Vec<u8>,
}

/// A message of the protocol.
#[derive(Debug, Clone)]
pub enum Message {
    /// HELLO.
    Hello(Hello),
    /// WELCOME.
    Welcome(Welcome),
    /// CALL.
    Call(Call),
    /// RESULT of a call.
    Result {
        /// The call.
        call: u64,
        /// What it gives.
        payload: Vec<u8>,
    },
    /// ERROR of a call.
    Error {
        /// The call.
        call: u64,
        /// The SQLSTATE, five characters.
        state: String,
        /// Why, for people.
        message: String,
    },
    /// CANCEL of a call.
    Cancel {
        /// The call.
        call: u64,
    },
    /// SHUTDOWN.
    Shutdown,
    /// SQL a call asks the host to run.
    Sql {
        /// The call.
        call: u64,
        /// Whether its rows are wanted.
        rows: bool,
        /// The statement.
        text: String,
        /// The values of its `?`s.
        args: Vec<Value>,
    },
    /// SETTINGS: the values of the settings the next call runs with, each
    /// with its name.
    Settings(Vec<(String, Value)>),
    /// SQLDONE: the rows of the SQL asked for.
    SqlDone {
        /// The call.
        call: u64,
        /// Its rows, none of a statement run.
        rows: Vec<Vec<Value>>,
    },
}

/// `rows`, counted, each its values.
///
/// # Errors
///
/// As `put_values`.
pub fn put_rows(out: &mut Vec<u8>, rows: &[Vec<Value>]) -> Result<()> {
    out.put_u32(u32::try_from(rows.len()).unwrap_or(u32::MAX));
    rows.iter().try_for_each(|row| put_values(out, row))
}

/// What `put_rows` wrote.
///
/// # Errors
///
/// SQLSTATE 38T03 if it is not that.
pub fn rows(r: &mut Reader<'_>) -> Result<Vec<Vec<Value>>> {
    (0..r.u32()?).map(|_| values(r)).collect()
}

impl Message {
    /// Its type byte.
    pub fn code(&self) -> u8 {
        match self {
            Message::Hello(_) => HELLO,
            Message::Welcome(_) => WELCOME,
            Message::Call(_) => CALL,
            Message::Result { .. } => RESULT,
            Message::Error { .. } => ERROR,
            Message::Cancel { .. } => CANCEL,
            Message::Shutdown => SHUTDOWN,
            Message::Sql { .. } => SQL,
            Message::SqlDone { .. } => SQL_DONE,
            Message::Settings(_) => SETTINGS,
        }
    }

    /// Its whole frame: its length, its type and its body.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T03 of a value no protocol 1 carries (`values.rs`).
    pub fn frame(&self) -> Result<Vec<u8>> {
        let mut body = vec![self.code()];
        self.put_body(&mut body)?;
        let mut out = Vec::with_capacity(body.len().saturating_add(LENGTH_BYTES));
        out.put_u32(u32::try_from(body.len()).map_err(|_| violation("a message longer than 4 GiB"))?);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Its body after the type byte.
    fn put_body(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            Message::Hello(h) => {
                out.put_u32(h.protocol);
                out.put_str(&h.extension);
                put_strings(out, &h.capabilities);
                for n in [h.max_message_bytes, h.max_call_bytes, h.memory_bytes] {
                    out.put_u64(n);
                }
            }
            Message::Welcome(w) => {
                out.put_u32(w.protocol);
                out.put_str(&w.name);
                out.put_str(&w.version);
                put_strings(out, &w.capabilities);
                out.put_bytes(&w.kinds);
            }
            Message::Call(c) => {
                out.put_u64(c.call);
                out.extend_from_slice(&[c.kind, c.op]);
                out.put_str(&c.object);
                out.extend_from_slice(&c.payload);
            }
            Message::Result { call, payload } => {
                out.put_u64(*call);
                out.extend_from_slice(payload);
            }
            Message::Error { call, state, message } => put_error(out, (*call, state, message)),
            Message::Cancel { call } => out.put_u64(*call),
            Message::Shutdown => {}
            Message::Sql { call, rows, text, args } => {
                out.put_u64(*call);
                out.put_u8(u8::from(*rows));
                out.put_str(text);
                put_values(out, args)?;
            }
            Message::SqlDone { call, rows } => {
                out.put_u64(*call);
                put_rows(out, rows)?;
            }
            Message::Settings(told) => {
                put_strings(out, &told.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>());
                put_values(out, &told.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>())?;
            }
        }
        Ok(())
    }

    /// The message of the body `body` of a frame, its type byte first.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T03 if it is no message of protocol 1.
    pub fn decode(body: &[u8]) -> Result<Message> {
        let (&code, rest) = body.split_first().ok_or_else(|| violation("an empty frame"))?;
        let mut r = Reader::received(rest, "plug-in message");
        let message = Self::decode_body(code, &mut r).map_err(|e| violation(&e.msg))?;
        Ok(message)
    }

    /// The message of type `code` `r` holds.
    fn decode_body(code: u8, r: &mut Reader<'_>) -> Result<Message> {
        let message = match code {
            HELLO => Message::Hello(Hello {
                protocol: r.u32()?,
                extension: r.str()?,
                capabilities: strings(r)?,
                max_message_bytes: r.u64()?,
                max_call_bytes: r.u64()?,
                memory_bytes: r.u64()?,
            }),
            WELCOME => Message::Welcome(Welcome {
                protocol: r.u32()?,
                name: r.str()?,
                version: r.str()?,
                capabilities: strings(r)?,
                kinds: r.bytes()?.to_vec(),
            }),
            CALL => {
                let call = r.u64()?;
                let (kind, op) = (r.u8()?, r.u8()?);
                Message::Call(Call { call, kind, op, object: r.str()?, payload: r.rest().to_vec() })
            }
            RESULT => Message::Result { call: r.u64()?, payload: r.rest().to_vec() },
            ERROR => {
                let call = r.u64()?;
                let state = String::from_utf8_lossy(r.take(SQLSTATE_BYTES)?).into_owned();
                Message::Error { call, state, message: r.str()? }
            }
            CANCEL => Message::Cancel { call: r.u64()? },
            SHUTDOWN => Message::Shutdown,
            SQL => Message::Sql { call: r.u64()?, rows: r.u8()? != 0, text: r.str()?, args: values(r)? },
            SQL_DONE => Message::SqlDone { call: r.u64()?, rows: rows(r)? },
            SETTINGS => {
                let (names, told) = (strings(r)?, values(r)?);
                if names.len() != told.len() {
                    return Err(violation("SETTINGS names as many settings as it gives values"));
                }
                Message::Settings(names.into_iter().zip(told).collect())
            }
            other => return Err(violation(&format!("no message is of type {other}"))),
        };
        r.finish()?;
        Ok(message)
    }
}

/// ERROR's body: a state of five characters, padded or cut.
fn put_error(out: &mut Vec<u8>, (call, state, message): (u64, &str, &str)) {
    out.put_u64(call);
    let mut bytes = [b'0'; SQLSTATE_BYTES];
    for (slot, b) in bytes.iter_mut().zip(state.bytes()) {
        *slot = b;
    }
    out.extend_from_slice(&bytes);
    out.put_str(message);
}

/// Writes `message` to `to`, and flushes it.
///
/// # Errors
///
/// SQLSTATE 38T01 if the pipe is closed, as the program ended.
pub fn write_message(to: &mut impl Write, message: &Message) -> Result<()> {
    let frame = message.frame()?;
    to.write_all(&frame).and_then(|()| to.flush()).map_err(|e| super::errors::ended(&format!("its input closed: {e}")))
}

/// The body of the next frame `from` holds, if any: none at its end.
///
/// # Errors
///
/// SQLSTATE 38T04 of a frame longer than `max` bytes, which is not read;
/// 38T01 if the pipe fails or ends within a frame.
pub fn read_frame(from: &mut impl Read, max: u64) -> Result<Option<Vec<u8>>> {
    let Some(length) = read_length(from, max)? else { return Ok(None) };
    read_body(from, length).map(Some)
}

/// The length of the next frame `from` holds, if any: none at its end.
///
/// # Errors
///
/// As `read_frame`.
pub fn read_length(from: &mut impl Read, max: u64) -> Result<Option<usize>> {
    let mut length = [0_u8; LENGTH_BYTES];
    if let Err(e) = from.read_exact(&mut length) {
        // Its end between frames is the program's end, not a failure.
        return if e.kind() == ErrorKind::UnexpectedEof { Ok(None) } else { Err(super::errors::ended(&format!("its output failed: {e}"))) };
    }
    let length = u32::from_le_bytes(length);
    if u64::from(length) > max {
        return Err(super::errors::too_long(u64::from(length), max));
    }
    Ok(Some(usize::try_from(length).unwrap_or(usize::MAX)))
}

/// The `length` bytes of a frame's body, its length read.
///
/// # Errors
///
/// SQLSTATE 38T01 if the pipe fails or ends first.
pub fn read_body(from: &mut impl Read, length: usize) -> Result<Vec<u8>> {
    let mut body = vec![0_u8; length];
    from.read_exact(&mut body).map_err(|e| super::errors::ended(&format!("its output ended within a message: {e}")))?;
    Ok(body)
}

#[cfg(test)]
mod tests;
