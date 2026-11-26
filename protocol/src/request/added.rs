//! \[DESIGN\] The requests version 1 gained after its first form, each sent
//! only where its capability was granted (RULES.md D185).
//!
//! ```text
//! 'Y' describe:   what u8 (0 a statement, 1 a cursor), statement u32,
//!                 cursor u32
//! 'G' ping
//! 'W' reset
//! 'U' change user: user, mechanism, initial bytes
//! 'I' copy in:    table, columns (strings), ends u8
//! 'J' copy data:  each row's values
//! 'N' copy end:   abandon u8
//! 'O' copy columns: as `columns.rs` says
//! 'V' debug:      as `debug.rs` says
//! ```

use turborel::bytes::{Put, Reader};
use turborel::error::Result;

use super::columns::{self, COPY_COLUMNS};
use super::{Described, Request};
use crate::DebugRequest;
use crate::fields::{counted, flag, put_count, put_flag, put_row, put_strs, row, strs};
use crate::frame;

const DESCRIBE: u8 = b'Y';
const PING: u8 = b'G';
const RESET: u8 = b'W';
const CHANGE_USER: u8 = b'U';
const COPY_IN: u8 = b'I';
const COPY_DATA: u8 = b'J';
const COPY_END: u8 = b'N';
const DEBUG: u8 = b'V';
/// Describe's `what` of a statement.
const STATEMENT: u8 = 0;
/// Describe's `what` of a cursor.
const CURSOR: u8 = 1;

impl Request {
    /// Appends this request, one added after version 1's first form,
    /// framed, to `out`; nothing for one of the first form.
    pub(super) fn encode_added(&self, out: &mut Vec<u8>) {
        match self {
            Request::Describe(described) => frame(out, DESCRIBE, |b| {
                let (what, statement, cursor) = match described {
                    Described::Statement(statement) => (STATEMENT, *statement, 0),
                    Described::Cursor { statement, cursor } => (CURSOR, *statement, *cursor),
                };
                b.put_u8(what);
                b.put_u32(statement);
                b.put_u32(cursor);
            }),
            Request::Ping => frame(out, PING, |_| ()),
            Request::Reset => frame(out, RESET, |_| ()),
            Request::ChangeUser { user, mechanism, initial } => frame(out, CHANGE_USER, |b| {
                b.put_str(user);
                b.put_str(mechanism);
                b.put_bytes(initial);
            }),
            Request::CopyIn { table, columns, ends } => frame(out, COPY_IN, |b| {
                b.put_str(table);
                put_strs(b, columns);
                put_flag(b, *ends);
            }),
            Request::CopyData(rows) => frame(out, COPY_DATA, |b| {
                put_count(b, rows.len());
                for each in rows {
                    put_row(b, each);
                }
            }),
            Request::CopyColumns(lanes) => frame(out, COPY_COLUMNS, |b| columns::put(b, lanes)),
            Request::CopyEnd { abandon } => frame(out, COPY_END, |b| put_flag(b, *abandon)),
            Request::Debug(debug) => frame(out, DEBUG, |b| debug.put(b)),
            Request::Hello { .. }
            | Request::Answer(_)
            | Request::Query { .. }
            | Request::Prepare { .. }
            | Request::Execute { .. }
            | Request::Fetch { .. }
            | Request::Close { .. }
            | Request::Deallocate { .. }
            | Request::Lob(_)
            | Request::Cancel(_)
            | Request::Terminate => {}
        }
    }
}

impl Request {
    /// It, of a Describe of a statement, if `cursor` is 0, as one's is.
    fn filter_cursor(self, cursor: u32) -> Option<Request> {
        (cursor == 0).then_some(self)
    }
}

/// The added request of kind `kind` `r` holds; none if no added request is
/// of that kind.
pub(super) fn decode(kind: u8, r: &mut Reader<'_>) -> Result<Option<Request>> {
    Ok(Some(match kind {
        DESCRIBE => {
            let (what, statement, cursor) = (r.u8()?, r.u32()?, r.u32()?);
            let refused = || r.corrupt("Describe describes a statement, 0, of cursor 0, or a cursor, 1");
            match what {
                STATEMENT => Request::Describe(Described::Statement(statement)).filter_cursor(cursor).ok_or_else(refused)?,
                CURSOR => Request::Describe(Described::Cursor { statement, cursor }),
                _ => return Err(refused()),
            }
        }
        PING => Request::Ping,
        RESET => Request::Reset,
        CHANGE_USER => Request::ChangeUser { user: r.str()?, mechanism: r.str()?, initial: r.bytes()?.to_vec() },
        COPY_IN => Request::CopyIn { table: r.str()?, columns: strs(r)?, ends: flag(r)? },
        COPY_DATA => Request::CopyData(counted(r, row)?),
        COPY_COLUMNS => Request::CopyColumns(columns::read(r)?),
        COPY_END => Request::CopyEnd { abandon: flag(r)? },
        DEBUG => Request::Debug(DebugRequest::read(r)?),
        _ => return Ok(None),
    }))
}
