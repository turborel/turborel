//! \[EXTENSION\] VR's messages as frames, and its state as the journal
//! keeps it: every integer little-endian, a log its length then each
//! record's length and bytes (`turborel::bytes`). The fields are [VRR
//! §4]'s, in its order; a frame malformed is refused (SQLCODE -919), and
//! a state malformed is corruption (-916), never a panic.

use turborel::bytes::{Put, Reader};
use turborel::error::Result;

use crate::ReplicaId;

/// What a frame is called in errors.
const FRAME: &str = "replication frame";
/// What VR's state is called in errors.
const STATE: &str = "replication state";
/// \[EXTENSION\] The format of VR's state: never changed while turborel is
/// alpha (its formats stay at 1).
const STATE_FORMAT: u8 = 1;

/// Tags of the messages, in \[VRR\]'s order; never reused.
const PREPARE: u8 = 1;
const PREPARE_OK: u8 = 2;
const COMMIT: u8 = 3;
const START_VIEW_CHANGE: u8 = 4;
const DO_VIEW_CHANGE: u8 = 5;
const START_VIEW: u8 = 6;
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

/// ⟨STARTVIEWCHANGE v, i⟩ [VRR §4.2 step 1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StartViewChange {
    pub(crate) view: u64,
    pub(crate) replica: ReplicaId,
}

/// ⟨DOVIEWCHANGE v, l, v′, n, k, i⟩ [VRR §4.2 step 2]: `normal_view` is
/// v′.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoViewChange {
    pub(crate) view: u64,
    pub(crate) log: Vec<Vec<u8>>,
    pub(crate) normal_view: u64,
    pub(crate) op: u64,
    pub(crate) commit: u64,
    pub(crate) replica: ReplicaId,
}

/// ⟨STARTVIEW v, l, n, k⟩ [VRR §4.2 step 3].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartView {
    pub(crate) view: u64,
    pub(crate) log: Vec<Vec<u8>>,
    pub(crate) op: u64,
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
    /// STARTVIEWCHANGE.
    StartViewChange(StartViewChange),
    /// DOVIEWCHANGE.
    DoViewChange(DoViewChange),
    /// STARTVIEW.
    StartView(StartView),
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
            Message::StartViewChange(m) => {
                put_u64s(&mut out, START_VIEW_CHANGE, &[m.view]);
                out.put_u32(m.replica.0);
            }
            Message::DoViewChange(m) => {
                put_u64s(&mut out, DO_VIEW_CHANGE, &[m.view, m.normal_view, m.op, m.commit]);
                out.put_u32(m.replica.0);
                put_log(&mut out, &m.log);
            }
            Message::StartView(m) => {
                put_u64s(&mut out, START_VIEW, &[m.view, m.op, m.commit]);
                put_log(&mut out, &m.log);
            }
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
            START_VIEW_CHANGE => Message::StartViewChange(StartViewChange { view: r.u64()?, replica: ReplicaId(r.u32()?) }),
            DO_VIEW_CHANGE => do_view_change(&mut r)?,
            START_VIEW => {
                let (view, op, commit) = (r.u64()?, r.u64()?, r.u64()?);
                Message::StartView(StartView { view, op, commit, log: log_through(&mut r, 0, op)? })
            }
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

/// The rest of a DOVIEWCHANGE, after its tag.
fn do_view_change(r: &mut Reader<'_>) -> Result<Message> {
    let (view, normal_view, op, commit) = (r.u64()?, r.u64()?, r.u64()?, r.u64()?);
    let replica = ReplicaId(r.u32()?);
    Ok(Message::DoViewChange(DoViewChange { view, normal_view, op, commit, replica, log: log_through(r, 0, op)? }))
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

/// \[EXTENSION\] What VR keeps durable of its own, beside the records: its
/// view-number, and the latest view in which its status was normal (v′).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ViewState {
    /// The view-number.
    pub(crate) view: u64,
    /// The latest view in which its status was normal.
    pub(crate) normal_view: u64,
}

impl ViewState {
    /// As `PersistState` writes it.
    pub(crate) fn encode(self) -> Vec<u8> {
        let mut out = Vec::new();
        put_u64s(&mut out, STATE_FORMAT, &[self.view, self.normal_view]);
        out
    }

    /// As a journal kept it.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is malformed, of another format, or names a
    /// normal view after its view.
    pub(crate) fn decode(bytes: &[u8]) -> Result<ViewState> {
        let mut r = Reader::new(bytes, STATE);
        if r.u8()? != STATE_FORMAT {
            return Err(r.corrupt("it is of no format known"));
        }
        let state = ViewState { view: r.u64()?, normal_view: r.u64()? };
        r.finish()?;
        if state.normal_view > state.view {
            return Err(r.corrupt("its normal view follows its view"));
        }
        Ok(state)
    }
}
