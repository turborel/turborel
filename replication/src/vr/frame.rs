//! \[EXTENSION\] VR's messages as frames: every integer little-endian, a
//! log its length then each record's length and bytes (`turborel::bytes`).
//! The fields are [VRR §4]'s, in its order; a frame malformed is refused
//! (SQLCODE -919), never a panic.

use turborel::bytes::{Put, Reader};
use turborel::error::Result;

use crate::ReplicaId;

/// What a frame is called in errors.
const FRAME: &str = "replication frame";

/// Tags of the messages, in \[VRR\]'s order; never reused. 4 to 6 are
/// the view change's.
const PREPARE: u8 = 1;
const PREPARE_OK: u8 = 2;
const COMMIT: u8 = 3;
const GET_STATE: u8 = 7;
const NEW_STATE: u8 = 8;

/// ⟨PREPARE v, m, n, k⟩ [VRR §4.1 step 3]: `record` is m, `op` n.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prepare {
    pub(crate) view: u64,
    pub(crate) record: Vec<u8>,
    pub(crate) op: u64,
    pub(crate) commit: u64,
}

/// ⟨PREPAREOK v, n, i⟩ [VRR §4.1 step 4].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PrepareOk {
    pub(crate) view: u64,
    pub(crate) op: u64,
    pub(crate) replica: ReplicaId,
}

/// ⟨COMMIT v, k⟩ [VRR §4.1 step 6].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Commit {
    pub(crate) view: u64,
    pub(crate) commit: u64,
}

/// ⟨GETSTATE v, n′, i⟩ [VRR §5.2]: `op` is n′.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GetState {
    pub(crate) view: u64,
    pub(crate) op: u64,
    pub(crate) replica: ReplicaId,
}

/// ⟨NEWSTATE v, l, n, k⟩ [VRR §5.2]; \[EXTENSION\] with `after`, the n′
/// of the GETSTATE it answers, after which `log` begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewState {
    pub(crate) view: u64,
    pub(crate) after: u64,
    pub(crate) log: Vec<Vec<u8>>,
    pub(crate) op: u64,
    pub(crate) commit: u64,
}

/// \[EXTENSION\] A message between replicas, as [VRR §4] names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Message {
    /// PREPARE.
    Prepare(Prepare),
    /// PREPAREOK.
    PrepareOk(PrepareOk),
    /// COMMIT.
    Commit(Commit),
    /// GETSTATE.
    GetState(GetState),
    /// NEWSTATE.
    NewState(NewState),
}

impl Message {
    /// The message as a frame.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Message::Prepare(m) => {
                put_u64s(&mut out, PREPARE, &[m.view, m.op, m.commit]);
                out.put_bytes(&m.record);
            }
            Message::PrepareOk(m) => {
                put_u64s(&mut out, PREPARE_OK, &[m.view, m.op]);
                out.put_u32(m.replica.0);
            }
            Message::Commit(m) => put_u64s(&mut out, COMMIT, &[m.view, m.commit]),
            Message::GetState(m) => {
                put_u64s(&mut out, GET_STATE, &[m.view, m.op]);
                out.put_u32(m.replica.0);
            }
            Message::NewState(m) => {
                put_u64s(&mut out, NEW_STATE, &[m.view, m.after, m.op, m.commit]);
                put_log(&mut out, &m.log);
            }
        }
        out
    }

    /// The message a frame holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -919 if the frame is malformed: of no tag, ending too soon,
    /// followed by more bytes, or with an op-number that is not its log's
    /// last position.
    pub(crate) fn decode(frame: &[u8]) -> Result<Message> {
        let mut r = Reader::received(frame, FRAME);
        let message = match r.u8()? {
            PREPARE => Message::Prepare(Prepare { view: r.u64()?, op: r.u64()?, commit: r.u64()?, record: r.bytes()?.to_vec() }),
            PREPARE_OK => Message::PrepareOk(PrepareOk { view: r.u64()?, op: r.u64()?, replica: ReplicaId(r.u32()?) }),
            COMMIT => Message::Commit(Commit { view: r.u64()?, commit: r.u64()? }),
            GET_STATE => Message::GetState(GetState { view: r.u64()?, op: r.u64()?, replica: ReplicaId(r.u32()?) }),
            NEW_STATE => {
                let (view, after, op, commit) = (r.u64()?, r.u64()?, r.u64()?, r.u64()?);
                Message::NewState(NewState { view, after, op, commit, log: log_through(&mut r, after, op)? })
            }
            other => return Err(r.corrupt(&format!("{other} is the tag of no message"))),
        };
        r.finish()?;
        Ok(message)
    }
}

/// A tag, then integers.
fn put_u64s(out: &mut Vec<u8>, tag: u8, values: &[u64]) {
    out.put_u8(tag);
    for value in values {
        out.put_u64(*value);
    }
}

/// A log: its length, then each record.
fn put_log(out: &mut Vec<u8>, log: &[Vec<u8>]) {
    out.put_u64(u64::try_from(log.len()).unwrap_or(u64::MAX));
    for record in log {
        out.put_bytes(record);
    }
}

/// A log of the records after `after` through `op`, refused if it holds
/// any other number. Each record is read before the next is kept, so a
/// damaged length cannot take memory the frame does not hold.
fn log_through(r: &mut Reader<'_>, after: u64, op: u64) -> Result<Vec<Vec<u8>>> {
    let length = r.u64()?;
    if after.checked_add(length) != Some(op) {
        return Err(r.corrupt("its op-number is not its log's last position"));
    }
    let mut log = Vec::new();
    for _ in 0..length {
        log.push(r.bytes()?.to_vec());
    }
    Ok(log)
}
