//! \[DESIGN\] What the host kinds' calls carry (RULES.md D229, D246 to
//! D254): each operation's code and its payloads. Replication's kinds
//! and the archive's are in `replication.rs`.
//!
//! ```text
//! kind                    op              CALL payload                               RESULT payload
//! 20 encoding             1 encode        context u64 | argument str | bytes         bytes
//!                         2 decode        context u64 | bytes                        bytes
//!                         3 accepts       argument str                               (nothing)
//! 21 hash function        1 hash          count u32 | bytes...                       digest bytes
//! 22 clock                1 now           (nothing)                                  microseconds i128
//! 23 log storage          1 exists        (nothing)                                  u8
//!                         2 open          (nothing)                                  found u8 | end u64
//!                         3 create        (nothing)                                  (nothing)
//!                         4 append        count u32 | (lsn u64 | bytes)...           (nothing), durable once answered
//!                         5 read          from u64 | max u32                         count u32 | (lsn u64 | bytes)... | more u8
//!                         6 truncate      before u64                                 (nothing)
//! 24 catalog storage      1 open          (nothing)                                  found u8 | last u8 | bytes if last
//!                         2 create        (nothing)                                  (nothing)
//!                         3 record        bytes | compact after u64                  (nothing), durable once answered
//! 25 spill                `spill.rs`
//! 26 file system          `files.rs`
//! 27 served connections   1 end           id u64                                     u8
//!                         2 cancel        id u64                                     u8
//! 28 listener             1 accept        (nothing)                                  peer str
//!                         2 relay         wait ms u32 | bytes to the client           closed u8 | bytes from it
//! 29 package crypto       1 sha256        bytes                                      32 bytes
//!                         2 verify        key 32 | message bytes | signature 64      u8
//! 30 keyring              1 key           name str                                   found u8 | 32 bytes if found
//! 31 background job       1 run           (nothing)                                  u8 0 asked, 1 again after | ms u64
//! ```

use crate::bytes::{Put, Reader};
use crate::error::Result;

/// 20 encoding: encode a page.
pub const ENCODING_ENCODE: u8 = 1;
/// 20 encoding: decode a page.
pub const ENCODING_DECODE: u8 = 2;
/// 20 encoding: whether an argument is taken.
pub const ENCODING_ACCEPTS: u8 = 3;
/// 21 hash function: a digest of parts.
pub const HASH_HASH: u8 = 1;
/// 22 clock: the time now.
pub const CLOCK_NOW: u8 = 1;
/// 23 log storage: whether the log is there.
pub const LOG_EXISTS: u8 = 1;
/// 23 log storage: open the log.
pub const LOG_OPEN: u8 = 2;
/// 23 log storage: create the log.
pub const LOG_CREATE: u8 = 3;
/// 23 log storage: append records, durably.
pub const LOG_APPEND: u8 = 4;
/// 23 log storage: read records from a position.
pub const LOG_READ: u8 = 5;
/// 23 log storage: drop records before a position.
pub const LOG_TRUNCATE: u8 = 6;
/// 24 catalog storage: open the store.
pub const CATALOG_OPEN: u8 = 1;
/// 24 catalog storage: create the store.
pub const CATALOG_CREATE: u8 = 2;
/// 24 catalog storage: record an image, durably.
pub const CATALOG_RECORD: u8 = 3;
/// 27 served connections: end one.
pub const SERVED_END: u8 = 1;
/// 27 served connections: cancel what one runs.
pub const SERVED_CANCEL: u8 = 2;
/// 28 listener: wait for the next connection.
pub const LISTENER_ACCEPT: u8 = 1;
/// 28 listener: relay bytes both ways.
pub const LISTENER_RELAY: u8 = 2;
/// 29 package crypto: a SHA-256 digest.
pub const CRYPTO_SHA256: u8 = 1;
/// 29 package crypto: check an Ed25519 signature.
pub const CRYPTO_VERIFY: u8 = 2;
/// 30 keyring: a key by name.
pub const KEYRING_KEY: u8 = 1;
/// 31 background job: run once.
pub const JOB_RUN: u8 = 1;

/// Writes `parts`, counted.
pub fn put_parts(out: &mut Vec<u8>, parts: &[&[u8]]) {
    out.put_u32(super::count(parts.len()));
    for part in parts {
        out.put_bytes(part);
    }
}

/// Reads counted parts.
///
/// # Errors
///
/// As the reader's.
pub fn parts(r: &mut Reader<'_>) -> Result<Vec<Vec<u8>>> {
    let n = r.u32()?;
    (0..n).map(|_| r.bytes().map(<[u8]>::to_vec)).collect()
}

/// Writes records, each with its position.
pub fn put_records(out: &mut Vec<u8>, records: &[(u64, Vec<u8>)]) {
    out.put_u32(super::count(records.len()));
    for (lsn, bytes) in records {
        out.put_u64(*lsn);
        out.put_bytes(bytes);
    }
}

/// Reads records, each with its position.
///
/// # Errors
///
/// As the reader's.
pub fn records(r: &mut Reader<'_>) -> Result<Vec<(u64, Vec<u8>)>> {
    let n = r.u32()?;
    (0..n).map(|_| Ok((r.u64()?, r.bytes()?.to_vec()))).collect()
}
