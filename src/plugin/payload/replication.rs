//! \[DESIGN\] The payloads of the host kinds replication and the archive
//! are served by (RULES.md D270 to D274): a replica transport (kind 32),
//! a journal (33), a state transfer (34), a consensus (35, its inputs and
//! outputs in `replication/consensus.rs`) and an archive (36, whose calls
//! are those of a backup target, kind 8).
//!
//! ```text
//! TRANSPORT    1 open         me u32 | configuration              -> members
//!              2 send         to u32 | frame bytes                -> sent u8 (0 to no member)
//!              3 receive      (nothing)                           -> count u32 | (from u32 | frame bytes)...
//!              4 reconfigure  configuration                       -> members
//! JOURNAL      1 open         (nothing)                           -> held
//!              2 persist      at u64 | record bytes               -> (nothing)
//!              3 truncate     after u64                           -> (nothing)
//!              4 retain       through u64                         -> (nothing)
//!              5 mark         at u64 | mark                       -> (nothing)
//!              6 state        state bytes                         -> (nothing), durable with every write before
//!              7 sync         (nothing)                           -> positions
//!              8 flush        (nothing)                           -> positions, every write durable
//! TRANSFER     1 begin        from u32                            -> fetch u64 | request
//!              2 take         fetch u64 | frame bytes             -> done u8 | request
//!              3 fetched      fetch u64                           -> manifest
//!              4 chunk        fetch u64 | file u32 | at u64 | max u32   -> bytes | digest u64
//!              5 serve        to u32 | export u64 | frame bytes | given -> served
//!              6 export       export u64 | manifest               -> (nothing)
//! configuration: count u32 | (id u32 | address str)...
//! members, positions: count u32 | u32... or u64...
//! mark: epoch u64 | configuration
//! held: durable u8 | state (u8 0, or 1 and bytes) | first u64 | count u32 | record bytes... | count u32 | (at u64 | mark)...
//! request: u8 0, or 1 and frame bytes
//! manifest: image bytes | count u32 | (within str | length u64 | owner str)...
//! given: u8 0, or 1 and file u32 | at u64 | bytes
//! served: u8 0 (no answer), 1 and answer bytes, or 2 and file u32 | at u64 | length u32 (a piece wanted)
//! ```
//!
//! SYNC and FLUSH answer the positions of the records made durable since
//! either last answered, each once, in the order persisted: the host
//! acknowledges a record only once one of them has named it. A chunk's
//! digest is `chunk_digest` of its bytes, which the host checks before it
//! stages them. SERVE answers a request out of the export EXPORT told the
//! program, asking the host for the pieces of its files it needs, one a
//! call, each given with the request again.

pub mod consensus;

use crate::bytes::{Put, Reader};
use crate::error::Result;

use super::count;

/// \[DESIGN\] The code of a replica transport, the host kind REPLICA
/// TRANSPORT.
pub const REPLICA_TRANSPORT: u8 = 32;
/// \[DESIGN\] The code of a journal, the host kind JOURNAL.
pub const JOURNAL: u8 = 33;
/// \[DESIGN\] The code of a state transfer, the host kind STATE TRANSFER.
pub const STATE_TRANSFER: u8 = 34;
/// \[DESIGN\] The code of a consensus, the host kind CONSENSUS.
pub const CONSENSUS: u8 = 35;
/// \[DESIGN\] The code of an archive, the host kind ARCHIVE: a backup
/// target's provider, before any database opens.
pub const ARCHIVE: u8 = 36;

/// A transport opened, linking a configuration.
pub const TRANSPORT_OPEN: u8 = 1;
/// A frame sent.
pub const TRANSPORT_SEND: u8 = 2;
/// The frames arrived.
pub const TRANSPORT_RECEIVE: u8 = 3;
/// The members linked anew.
pub const TRANSPORT_RECONFIGURE: u8 = 4;

/// A journal opened: what it holds.
pub const JOURNAL_OPEN: u8 = 1;
/// A record written.
pub const JOURNAL_PERSIST: u8 = 2;
/// The records after a position removed.
pub const JOURNAL_TRUNCATE: u8 = 3;
/// The records through a position let go.
pub const JOURNAL_RETAIN: u8 = 4;
/// A record marked a reconfiguration.
pub const JOURNAL_MARK: u8 = 5;
/// The protocol's state written, durable with every write before.
pub const JOURNAL_STATE: u8 = 6;
/// The records made durable since last asked.
pub const JOURNAL_SYNC: u8 = 7;
/// Every write made durable, and the records so.
pub const JOURNAL_FLUSH: u8 = 8;

/// A fetch begun.
pub const TRANSFER_BEGIN: u8 = 1;
/// An answer taken by a fetch.
pub const TRANSFER_TAKE: u8 = 2;
/// The manifest of the export fetched.
pub const TRANSFER_FETCHED: u8 = 3;
/// A chunk of a file of the export fetched.
pub const TRANSFER_CHUNK: u8 = 4;
/// A request answered out of an export.
pub const TRANSFER_SERVE: u8 = 5;
/// The export served, told.
pub const TRANSFER_EXPORT: u8 = 6;

/// SERVE's outcome: no answer, the frame being no request.
pub const SERVED_NONE: u8 = 0;
/// SERVE's outcome: the answer to send.
pub const SERVED_ANSWER: u8 = 1;
/// SERVE's outcome: a piece of a file wanted first.
pub const SERVED_NEED: u8 = 2;

/// A member of a group, as the payloads carry it.
pub type Member = (u32, String);

/// What a journal held as it opened: the protocol's last state, the
/// position before its first record, the records, and the marks among them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
    /// Whether what it makes durable survives a crash.
    pub durable: bool,
    /// The last state written, if any.
    pub state: Option<Vec<u8>>,
    /// The position before the first record.
    pub first: u64,
    /// The records, from the position after `first` on.
    pub records: Vec<Vec<u8>>,
    /// The marks, each its record's position, its epoch and its members.
    pub marks: Vec<(u64, u64, Vec<Member>)>,
}

/// An export's manifest: its image, and each file's path within the
/// database's directory, length and owner, empty for the database's own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// The image.
    pub image: Vec<u8>,
    /// The files.
    pub files: Vec<(String, u64, String)>,
}

/// What SERVE answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Served {
    /// No answer: the frame is no request of the transfer.
    Nothing,
    /// The answer to send.
    Answer(Vec<u8>),
    /// A piece of a file the answer needs first: its file, where, how long.
    Need(u32, u64, u32),
}

/// \[DESIGN\] The digest of a chunk: XXH3-64 of its bytes, seed 0, which
/// the host checks before it stages them.
pub fn chunk_digest(bytes: &[u8]) -> u64 {
    crate::hash::xxhash::xxh3_64(bytes)
}

/// A configuration.
pub fn put_configuration(out: &mut Vec<u8>, members: &[Member]) {
    out.put_u32(count(members.len()));
    for (id, address) in members {
        out.put_u32(*id);
        out.put_str(address);
    }
}

/// What `put_configuration` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn configuration(r: &mut Reader<'_>) -> Result<Vec<Member>> {
    (0..r.u32()?).map(|_| Ok((r.u32()?, r.str()?))).collect()
}

/// Members, by number.
pub fn put_members(out: &mut Vec<u8>, members: &[u32]) {
    out.put_u32(count(members.len()));
    for id in members {
        out.put_u32(*id);
    }
}

/// What `put_members` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn members(r: &mut Reader<'_>) -> Result<Vec<u32>> {
    (0..r.u32()?).map(|_| r.u32()).collect()
}

/// Positions.
pub fn put_positions(out: &mut Vec<u8>, positions: &[u64]) {
    out.put_u32(count(positions.len()));
    for at in positions {
        out.put_u64(*at);
    }
}

/// What `put_positions` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn positions(r: &mut Reader<'_>) -> Result<Vec<u64>> {
    (0..r.u32()?).map(|_| r.u64()).collect()
}

/// Frames arrived, each from its replica.
pub fn put_arrived(out: &mut Vec<u8>, arrived: &[(u32, Vec<u8>)]) {
    out.put_u32(count(arrived.len()));
    for (from, frame) in arrived {
        out.put_u32(*from);
        out.put_bytes(frame);
    }
}

/// What `put_arrived` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn arrived(r: &mut Reader<'_>) -> Result<Vec<(u32, Vec<u8>)>> {
    (0..r.u32()?).map(|_| Ok((r.u32()?, r.bytes()?.to_vec()))).collect()
}

/// Bytes that may be absent.
pub fn put_optional(out: &mut Vec<u8>, bytes: Option<&[u8]>) {
    match bytes {
        Some(b) => {
            out.put_u8(1);
            out.put_bytes(b);
        }
        None => out.put_u8(0),
    }
}

/// What `put_optional` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn optional(r: &mut Reader<'_>) -> Result<Option<Vec<u8>>> {
    if r.u8()? == 0 { Ok(None) } else { r.bytes().map(|b| Some(b.to_vec())) }
}

/// A mark: its epoch and members.
pub fn put_mark(out: &mut Vec<u8>, epoch: u64, members: &[Member]) {
    out.put_u64(epoch);
    put_configuration(out, members);
}

/// What `put_mark` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn mark(r: &mut Reader<'_>) -> Result<(u64, Vec<Member>)> {
    Ok((r.u64()?, configuration(r)?))
}

/// OPEN's answer of a journal.
pub fn put_held(out: &mut Vec<u8>, held: &Held) {
    out.put_u8(u8::from(held.durable));
    put_optional(out, held.state.as_deref());
    out.put_u64(held.first);
    out.put_u32(count(held.records.len()));
    for record in &held.records {
        out.put_bytes(record);
    }
    out.put_u32(count(held.marks.len()));
    for (at, epoch, members) in &held.marks {
        out.put_u64(*at);
        put_mark(out, *epoch, members);
    }
}

/// What `put_held` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn held(r: &mut Reader<'_>) -> Result<Held> {
    let durable = r.u8()? != 0;
    let state = optional(r)?;
    let first = r.u64()?;
    let records = (0..r.u32()?).map(|_| r.bytes().map(<[u8]>::to_vec)).collect::<Result<Vec<_>>>()?;
    let marks = (0..r.u32()?)
        .map(|_| {
            let at = r.u64()?;
            let (epoch, members) = mark(r)?;
            Ok((at, epoch, members))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Held { durable, state, first, records, marks })
}

/// An export's manifest.
pub fn put_manifest(out: &mut Vec<u8>, manifest: &Manifest) {
    out.put_bytes(&manifest.image);
    out.put_u32(count(manifest.files.len()));
    for (within, length, owner) in &manifest.files {
        out.put_str(within);
        out.put_u64(*length);
        out.put_str(owner);
    }
}

/// What `put_manifest` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn manifest(r: &mut Reader<'_>) -> Result<Manifest> {
    let image = r.bytes()?.to_vec();
    let files = (0..r.u32()?).map(|_| Ok((r.str()?, r.u64()?, r.str()?))).collect::<Result<Vec<_>>>()?;
    Ok(Manifest { image, files })
}

/// A piece given SERVE: its file, where, its bytes.
pub fn put_given(out: &mut Vec<u8>, given: Option<(u32, u64, &[u8])>) {
    match given {
        Some((file, at, bytes)) => {
            out.put_u8(1);
            out.put_u32(file);
            out.put_u64(at);
            out.put_bytes(bytes);
        }
        None => out.put_u8(0),
    }
}

/// What `put_given` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn given(r: &mut Reader<'_>) -> Result<Option<(u32, u64, Vec<u8>)>> {
    if r.u8()? == 0 {
        return Ok(None);
    }
    Ok(Some((r.u32()?, r.u64()?, r.bytes()?.to_vec())))
}

/// SERVE's answer.
pub fn put_served(out: &mut Vec<u8>, served: &Served) {
    match served {
        Served::Nothing => out.put_u8(SERVED_NONE),
        Served::Answer(answer) => {
            out.put_u8(SERVED_ANSWER);
            out.put_bytes(answer);
        }
        Served::Need(file, at, length) => {
            out.put_u8(SERVED_NEED);
            out.put_u32(*file);
            out.put_u64(*at);
            out.put_u32(*length);
        }
    }
}

/// What `put_served` wrote.
///
/// # Errors
///
/// `Reader`'s; malformed if the outcome is none of the three.
pub fn served(r: &mut Reader<'_>) -> Result<Served> {
    match r.u8()? {
        SERVED_NONE => Ok(Served::Nothing),
        SERVED_ANSWER => Ok(Served::Answer(r.bytes()?.to_vec())),
        SERVED_NEED => Ok(Served::Need(r.u32()?, r.u64()?, r.u32()?)),
        other => Err(r.corrupt(&format!("no outcome of SERVE is {other}"))),
    }
}

#[cfg(test)]
mod tests;
