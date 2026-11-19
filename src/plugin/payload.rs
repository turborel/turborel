//! \[DESIGN\] What each kind's calls carry in protocol 1 (RULES.md D167):
//! the operations of each kind and the payloads of their CALL and RESULT.
//!
//! ```text
//! kind              op            CALL payload                          RESULT payload
//! 1 scalar          1 call        rows                                  values, one a row
//! 2 aggregate       1 step        state bytes | values                  state bytes
//!                   2 merge       state bytes | state bytes             state bytes
//!                   3 finish      state bytes                           value
//! 3 table function  1 rows        values | read rows (count u32, rows)  rows
//!                   2 reads       values (null where unknown)           strs (count u32)
//! 4 procedure       1 call        values; SQL frames may come between   (nothing)
//! 5 metric          1 distances   vector | vectors (count u32)          values, one a vector
//! 6 embedding       1 embed       strs (count u32)                      values, each a vector
//! 7 authenticator   1 begin       user str | peer str | local u8 | secure u8   exchange u64
//!                   2 step        exchange u64 | bytes                  outcome u8, and a challenge's bytes or a refusal's str
//! 8 backup target   1 open        location str                          target u64
//!                   2 begin       target u64 | object str               upload u64
//!                   3 abandon     target u64 | upload u64               (nothing)
//!                   4 write       target u64 | upload u64 | bytes       (nothing)
//!                   5 finish      target u64 | upload u64               (nothing)
//!                   6 read        target u64 | object str | offset u64 | max u32   bytes (empty at the end)
//!                   7 list        target u64 | prefix str               strs
//!                   8 remove      target u64 | object str               (nothing)
//! 9 partition       1 check key   types (strs)                          (nothing)
//!                   2 bound       types | bound                         bound
//!                   3 overlaps    bound | bound                         u8
//!                   4 contains    bound | values                        u8
//!                   5 route       bounds (count u32) | values           u8 0, or 1 and index u32
//!                   6 admits      bound | restrictions (count u32)      u8
//! ```
//!
//! An aggregate's state is the program's own bytes, kept by the host, so
//! a program holds nothing between calls. An exchange, a target and an
//! upload are the program's, so the host keeps one process for each.

use crate::bytes::{Put, Reader};
use crate::error::Result;

/// The operation of a scalar function's call.
pub const SCALAR_CALL: u8 = 1;
/// An aggregate's step: values taken into a state.
pub const AGGREGATE_STEP: u8 = 1;
/// An aggregate's merge of two states.
pub const AGGREGATE_MERGE: u8 = 2;
/// An aggregate's result of a state.
pub const AGGREGATE_FINISH: u8 = 3;
/// A table function's rows.
pub const TABLE_ROWS: u8 = 1;
/// The queries a table function reads.
pub const TABLE_READS: u8 = 2;
/// A procedure's call.
pub const PROCEDURE_CALL: u8 = 1;
/// A metric's distances of one vector to many.
pub const METRIC_DISTANCES: u8 = 1;
/// A model's vectors of texts.
pub const EMBED: u8 = 1;
/// An authenticator's exchange begun.
pub const AUTH_BEGIN: u8 = 1;
/// An authenticator's next step.
pub const AUTH_STEP: u8 = 2;
/// The outcome of a step: bytes to send the client.
pub const AUTH_CHALLENGE: u8 = 0;
/// The outcome of a step: proven, the user's name.
pub const AUTH_ACCEPT: u8 = 1;
/// The outcome of a step: not proven, why.
pub const AUTH_REFUSE: u8 = 2;
/// A backup target opened at a location.
pub const TARGET_OPEN: u8 = 1;
/// An upload of an object begun.
pub const TARGET_BEGIN: u8 = 2;
/// An upload abandoned, leaving no object.
pub const TARGET_ABANDON: u8 = 3;
/// Bytes written to an object.
pub const TARGET_WRITE: u8 = 4;
/// An object written, made durable.
pub const TARGET_FINISH: u8 = 5;
/// Bytes read from an object.
pub const TARGET_READ: u8 = 6;
/// The objects of a target of a prefix.
pub const TARGET_LIST: u8 = 7;
/// An object removed.
pub const TARGET_REMOVE: u8 = 8;
/// Whether a key of these types is divided by the strategy.
pub const PARTITION_CHECK_KEY: u8 = 1;
/// A bound checked and normalized.
pub const PARTITION_BOUND: u8 = 2;
/// Whether two bounds overlap.
pub const PARTITION_OVERLAPS: u8 = 3;
/// Whether a bound holds a key.
pub const PARTITION_CONTAINS: u8 = 4;
/// The bound of several holding a key.
pub const PARTITION_ROUTE: u8 = 5;
/// Whether a bound may hold a row meeting restrictions.
pub const PARTITION_ADMITS: u8 = 6;

/// Strings, counted.
pub fn put_strings(out: &mut Vec<u8>, all: &[String]) {
    out.put_u32(u32::try_from(all.len()).unwrap_or(u32::MAX));
    for s in all {
        out.put_str(s);
    }
}

/// What `put_strings` wrote.
///
/// # Errors
///
/// `Reader`'s, if it is not that.
pub fn strings(r: &mut Reader<'_>) -> Result<Vec<String>> {
    (0..r.u32()?).map(|_| r.str()).collect()
}

/// A reader of a RESULT's payload `payload`.
pub fn reader(payload: &[u8]) -> Reader<'_> {
    Reader::received(payload, "plug-in result")
}
