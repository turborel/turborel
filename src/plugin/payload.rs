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
//! 10 ranking        1 weights     `ranking.rs`
//!                   2 bounds
//! 11 quantizer      1 train       `quantizer.rs`
//! 12 collation      1 keys        `collation.rs`
//! 13 system table   1 rows        `system.rs`
//! 14 XQuery         1 call        rows, one row                         values, one
//! 15 type           1 input       `types.rs`
//!                   2 receive
//! 16 engine         1 create .. 7 drop   `engine.rs`
//!                   8 statistics  `statistics.rs`
//! 17 index method   1 create .. 8 drop   `index.rs`
//!                   9 statistics  `statistics.rs`
//! 18 credentials    1 verifier    `process.rs`
//! 19 diagnostics    1 events      `process.rs`
//! 37 optimizer rule 1 rewrite     plan form (`plan.rs`)             u8 0 (no change), or u8 1 | plan form
//! ```
//!
//! The kinds from 10 (RULES.md D220) each have a module of their own,
//! which says their payloads; no comparison, hash or search step of them
//! is a call of its own, but a batch of them.
//!
//! An aggregate's state is the program's own bytes, kept by the host, so
//! a program holds nothing between calls. An exchange, a target and an
//! upload are the program's, so the host keeps one process for each.

pub mod collation;
pub mod engine;
pub mod eviction;
pub mod hosted;
pub mod index;
pub mod process;
pub mod quantizer;
pub mod ranking;
pub mod replication;
pub mod statistics;
pub mod system;
pub mod types;

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

/// A ranking's weights of terms in their documents (`ranking.rs`).
pub const RANKING_WEIGHTS: u8 = 1;
/// A ranking's bounds of terms.
pub const RANKING_BOUNDS: u8 = 2;
/// A quantizer's training (`quantizer.rs`).
pub const QUANTIZER_TRAIN: u8 = 1;
/// A collation's sort keys of strings (`collation.rs`).
pub const COLLATION_KEYS: u8 = 1;
/// A system table's page of rows (`system.rs`).
pub const SYSTEM_ROWS: u8 = 1;
/// An XQuery function's call: rows, as a scalar function's.
pub const XQUERY_CALL: u8 = 1;
/// A type's values made of their text forms (`types.rs`).
pub const TYPE_INPUT: u8 = 1;
/// A type's values made of their binary forms.
pub const TYPE_RECEIVE: u8 = 2;
/// The verifier of a password (`process.rs`).
pub const CREDENTIALS_VERIFIER: u8 = 1;
/// Events told a sink.
pub const DIAGNOSTICS_EVENTS: u8 = 1;

/// A storage engine's new table (`engine.rs`).
pub const ENGINE_CREATE: u8 = 1;
/// A table opened again.
pub const ENGINE_OPEN: u8 = 2;
/// A page of a table's rows, in order of id.
pub const ENGINE_SCAN: u8 = 3;
/// Rows by id.
pub const ENGINE_GET: u8 = 4;
/// Changes made, all or none.
pub const ENGINE_APPLY: u8 = 5;
/// The changes applied made durable.
pub const ENGINE_CHECKPOINT: u8 = 6;
/// A table dropped.
pub const ENGINE_DROP: u8 = 7;

/// An index method's new index (`index.rs`).
pub const INDEX_CREATE: u8 = 1;
/// An index opened again.
pub const INDEX_OPEN: u8 = 2;
/// Entries inserted and removed, in order.
pub const INDEX_WRITE: u8 = 3;
/// A page of the entries a probe selects.
pub const INDEX_SEARCH: u8 = 4;
/// The rows of a key.
pub const INDEX_ROWS: u8 = 5;
/// The nearest entries.
pub const INDEX_NEAREST: u8 = 6;
/// The index made durable and clean.
pub const INDEX_CHECKPOINT: u8 = 7;
/// An index dropped.
pub const INDEX_DROP: u8 = 8;
/// What a table's engine knows of its rows (`statistics.rs`, RULES.md D278).
pub const ENGINE_STATISTICS: u8 = 8;
/// What an index's method knows of its entries (`statistics.rs`).
pub const INDEX_STATISTICS: u8 = 9;
/// An optimizer rule's rewrite of a plan form (RULES.md D275).
pub const RULE_REWRITE: u8 = 1;

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

/// A count, as a u32.
pub fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A float, as its bits.
pub fn put_f64(out: &mut Vec<u8>, x: f64) {
    out.put_u64(x.to_bits());
}

/// What `put_f64` wrote.
///
/// # Errors
///
/// `Reader`'s, if it holds none.
pub fn read_f64(r: &mut Reader<'_>) -> Result<f64> {
    r.u64().map(f64::from_bits)
}

/// Floats, counted.
pub fn put_f64s(out: &mut Vec<u8>, all: &[f64]) {
    out.put_u32(count(all.len()));
    for x in all {
        put_f64(out, *x);
    }
}

/// What `put_f64s` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn f64s(r: &mut Reader<'_>) -> Result<Vec<f64>> {
    (0..r.u32()?).map(|_| read_f64(r)).collect()
}

/// Single floats, counted, each as its bits, a u32.
pub fn put_f32s(out: &mut Vec<u8>, all: &[f32]) {
    out.put_u32(count(all.len()));
    for x in all {
        out.put_u32(x.to_bits());
    }
}

/// What `put_f32s` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn f32s(r: &mut Reader<'_>) -> Result<Vec<f32>> {
    (0..r.u32()?).map(|_| r.u32().map(f32::from_bits)).collect()
}
