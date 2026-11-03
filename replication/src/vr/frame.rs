//! \[EXTENSION\] VR's messages as frames, and its state as the journal
//! keeps it: every integer little-endian, a log its length then each
//! record's length and bytes (`turborel::bytes`), a window after the
//! position before it (RULES.md D57, log retention), then the
//! reconfigurations marked among its records, each its position and the
//! next epoch's members [VRR §7]. Every frame begins with its tag and the
//! sender's epoch. The fields are [VRR §4]'s, in its order; a frame
//! malformed is refused (SQLCODE -919), and a state malformed is
//! corruption (-916), never a panic.

use std::collections::BTreeMap;

use turborel::bytes::{Put, Reader};
use turborel::error::Result;

use crate::journal::{put_configuration, put_mark, read_configuration, read_mark};
use crate::{Configuration, Mark, ReplicaId};

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
/// \[EXTENSION\] NEEDSTATE: the state asked for is behind what the answerer
/// retains of its log, so the asker fetches a state transfer (RULES.md
/// D57).
const NEED_STATE: u8 = 9;
const RECOVERY: u8 = 10;
const RECOVERY_RESPONSE: u8 = 11;
const EPOCH_STARTED: u8 = 12;

/// The reconfigurations marked in a log, by position.
pub(crate) type Marks = BTreeMap<u64, Mark>;

/// ⟨PREPARE v, m, n, k⟩ [VRR §4.1 step 3]: `record` is m, `op` n;
/// \[EXTENSION\] `mark`, the epoch the record begins and its members if
/// the record is a reconfiguration [VRR §7].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prepare {
    pub(crate) view: u64,
    pub(crate) record: Vec<u8>,
    pub(crate) op: u64,
    pub(crate) commit: u64,
    pub(crate) mark: Option<Mark>,
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
/// v′; \[EXTENSION\] `log` the sender's window, after `after` (RULES.md
/// D57, log retention), `marks` its reconfigurations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoViewChange {
    pub(crate) view: u64,
    pub(crate) after: u64,
    pub(crate) log: Vec<Vec<u8>>,
    pub(crate) marks: Marks,
    pub(crate) normal_view: u64,
    pub(crate) op: u64,
    pub(crate) commit: u64,
    pub(crate) replica: ReplicaId,
}

/// ⟨STARTVIEW v, l, n, k⟩ [VRR §4.2 step 3]; \[EXTENSION\] `log` the
/// primary's window, after `after`, `marks` its reconfigurations, and
/// `configuration` the epoch's members, for a replica behind the epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartView {
    pub(crate) view: u64,
    pub(crate) after: u64,
    pub(crate) log: Vec<Vec<u8>>,
    pub(crate) marks: Marks,
    pub(crate) op: u64,
    pub(crate) commit: u64,
    pub(crate) configuration: Configuration,
    /// The position of the mark the epoch began at.
    pub(crate) began: u64,
}

/// \[EXTENSION\] ⟨NEEDSTATE v, n′⟩: the answer to a GETSTATE for the
/// position n′, before what the answerer retains, so the asker fetches a
/// state transfer of its database first (RULES.md D57); `configuration`
/// the answerer's epoch's members and `began` the position of the mark
/// it began at, which an asker of an earlier epoch adopts before it
/// fetches [VRR §7].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NeedState {
    pub(crate) view: u64,
    pub(crate) op: u64,
    pub(crate) configuration: Configuration,
    /// The position of the mark the epoch began at.
    pub(crate) began: u64,
}

/// ⟨GETSTATE v, n′, i⟩ [VRR §5.2]: `op` is n′.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GetState {
    pub(crate) view: u64,
    pub(crate) op: u64,
    pub(crate) replica: ReplicaId,
}

/// ⟨NEWSTATE v, l, n, k⟩ [VRR §5.2]; \[EXTENSION\] with `after`, the n′
/// of the GETSTATE it answers, after which `log` begins, `marks` its
/// reconfigurations, and `configuration` the epoch's members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewState {
    pub(crate) view: u64,
    pub(crate) after: u64,
    pub(crate) log: Vec<Vec<u8>>,
    pub(crate) marks: Marks,
    pub(crate) op: u64,
    pub(crate) commit: u64,
    pub(crate) configuration: Configuration,
    /// The position of the mark the epoch began at.
    pub(crate) began: u64,
}

/// ⟨RECOVERY i, x⟩ [VRR §4.3]: `nonce` is x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Recovery {
    pub(crate) replica: ReplicaId,
    pub(crate) nonce: u64,
}

/// A primary's log as a RECOVERYRESPONSE carries it: l, n and k of
/// [VRR §4.3]; \[EXTENSION\] `log` its window, after `after`, `marks` its
/// reconfigurations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrimaryLog {
    pub(crate) after: u64,
    pub(crate) log: Vec<Vec<u8>>,
    pub(crate) marks: Marks,
    pub(crate) op: u64,
    pub(crate) commit: u64,
}

/// ⟨RECOVERYRESPONSE v, x, l, n, k, j⟩ [VRR §4.3]: `log` is l, n and k,
/// which a primary alone gives; \[EXTENSION\] `configuration` the
/// answerer's epoch's members, which the recovering replica adopts with
/// the epoch (RULES.md D57).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveryResponse {
    pub(crate) view: u64,
    pub(crate) nonce: u64,
    pub(crate) replica: ReplicaId,
    pub(crate) configuration: Configuration,
    /// The position of the mark the answerer's epoch began at.
    pub(crate) began: u64,
    pub(crate) log: Option<PrimaryLog>,
}

/// ⟨EPOCHSTARTED e, i⟩ [VRR §7]: replica `replica` is in the epoch the
/// frame names, told to the replicas of the old epoch that are no members
/// of it, and to a replica behind the epoch; `configuration` the epoch's
/// members, by which one behind reaches the sender, new to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EpochStarted {
    pub(crate) replica: ReplicaId,
    pub(crate) configuration: Configuration,
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
    /// NEEDSTATE.
    NeedState(NeedState),
    /// RECOVERY.
    Recovery(Recovery),
    /// RECOVERYRESPONSE.
    RecoveryResponse(RecoveryResponse),
    /// EPOCHSTARTED.
    EpochStarted(EpochStarted),
}

impl Message {
    /// The message's name, for diagnostics.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Message::Prepare(_) => "PREPARE",
            Message::PrepareOk(_) => "PREPAREOK",
            Message::Commit(_) => "COMMIT",
            Message::StartViewChange(_) => "STARTVIEWCHANGE",
            Message::DoViewChange(_) => "DOVIEWCHANGE",
            Message::StartView(_) => "STARTVIEW",
            Message::GetState(_) => "GETSTATE",
            Message::NewState(_) => "NEWSTATE",
            Message::NeedState(_) => "NEEDSTATE",
            Message::Recovery(_) => "RECOVERY",
            Message::RecoveryResponse(_) => "RECOVERYRESPONSE",
            Message::EpochStarted(_) => "EPOCHSTARTED",
        }
    }

    /// The message as a frame, sent in `epoch`.
    pub(crate) fn encode(&self, epoch: u64) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Message::Prepare(m) => {
                put_u64s(&mut out, PREPARE, epoch, &[m.view, m.op, m.commit]);
                out.put_bytes(&m.record);
                put_optional(&mut out, m.mark.as_ref());
            }
            Message::PrepareOk(m) => {
                put_u64s(&mut out, PREPARE_OK, epoch, &[m.view, m.op]);
                out.put_u32(m.replica.0);
            }
            Message::Commit(m) => put_u64s(&mut out, COMMIT, epoch, &[m.view, m.commit]),
            Message::StartViewChange(m) => {
                put_u64s(&mut out, START_VIEW_CHANGE, epoch, &[m.view]);
                out.put_u32(m.replica.0);
            }
            Message::DoViewChange(m) => put_do_view_change(&mut out, epoch, m),
            Message::StartView(m) => {
                put_u64s(&mut out, START_VIEW, epoch, &[m.view, m.after, m.op, m.commit, m.began]);
                put_log(&mut out, &m.log, &m.marks);
                put_configuration(&mut out, &m.configuration);
            }
            Message::GetState(m) => {
                put_u64s(&mut out, GET_STATE, epoch, &[m.view, m.op]);
                out.put_u32(m.replica.0);
            }
            Message::NewState(m) => {
                put_u64s(&mut out, NEW_STATE, epoch, &[m.view, m.after, m.op, m.commit, m.began]);
                put_log(&mut out, &m.log, &m.marks);
                put_configuration(&mut out, &m.configuration);
            }
            Message::NeedState(m) => {
                put_u64s(&mut out, NEED_STATE, epoch, &[m.view, m.op, m.began]);
                put_configuration(&mut out, &m.configuration);
            }
            Message::Recovery(m) => {
                put_u64s(&mut out, RECOVERY, epoch, &[m.nonce]);
                out.put_u32(m.replica.0);
            }
            Message::RecoveryResponse(m) => put_recovery_response(&mut out, epoch, m),
            Message::EpochStarted(m) => {
                put_u64s(&mut out, EPOCH_STARTED, epoch, &[]);
                out.put_u32(m.replica.0);
                put_configuration(&mut out, &m.configuration);
            }
        }
        out
    }

    /// The message a frame holds, and the epoch it was sent in.
    ///
    /// # Errors
    ///
    /// SQLCODE -919 if the frame is malformed: of no tag, ending too soon,
    /// followed by more bytes, or with an op-number that is not its log's
    /// last position.
    pub(crate) fn decode(frame: &[u8]) -> Result<(u64, Message)> {
        let mut r = Reader::received(frame, FRAME);
        let tag = r.u8()?;
        let epoch = r.u64()?;
        let message = match tag {
            PREPARE => {
                let (view, op, commit) = (r.u64()?, r.u64()?, r.u64()?);
                let record = r.bytes()?.to_vec();
                Message::Prepare(Prepare { view, record, op, commit, mark: read_optional(&mut r)? })
            }
            PREPARE_OK => Message::PrepareOk(PrepareOk { view: r.u64()?, op: r.u64()?, replica: ReplicaId(r.u32()?) }),
            COMMIT => Message::Commit(Commit { view: r.u64()?, commit: r.u64()? }),
            START_VIEW_CHANGE => Message::StartViewChange(StartViewChange { view: r.u64()?, replica: ReplicaId(r.u32()?) }),
            DO_VIEW_CHANGE => do_view_change(&mut r)?,
            START_VIEW => {
                let (view, after, op, commit, began) = (r.u64()?, r.u64()?, r.u64()?, r.u64()?, r.u64()?);
                let (log, marks) = log_through(&mut r, after, op)?;
                Message::StartView(StartView { view, after, op, commit, began, log, marks, configuration: read_configuration(&mut r)? })
            }
            GET_STATE => Message::GetState(GetState { view: r.u64()?, op: r.u64()?, replica: ReplicaId(r.u32()?) }),
            NEW_STATE => {
                let (view, after, op, commit, began) = (r.u64()?, r.u64()?, r.u64()?, r.u64()?, r.u64()?);
                let (log, marks) = log_through(&mut r, after, op)?;
                Message::NewState(NewState { view, after, op, commit, began, log, marks, configuration: read_configuration(&mut r)? })
            }
            NEED_STATE => {
                let (view, op, began) = (r.u64()?, r.u64()?, r.u64()?);
                Message::NeedState(NeedState { view, op, began, configuration: read_configuration(&mut r)? })
            }
            RECOVERY => Message::Recovery(Recovery { nonce: r.u64()?, replica: ReplicaId(r.u32()?) }),
            RECOVERY_RESPONSE => recovery_response(&mut r)?,
            EPOCH_STARTED => {
                let replica = ReplicaId(r.u32()?);
                Message::EpochStarted(EpochStarted { replica, configuration: read_configuration(&mut r)? })
            }
            other => return Err(r.corrupt(&format!("{other} is the tag of no message"))),
        };
        r.finish()?;
        Ok((epoch, message))
    }
}

/// The rest of a DOVIEWCHANGE, after its tag and epoch.
fn do_view_change(r: &mut Reader<'_>) -> Result<Message> {
    let (view, after, normal_view, op, commit) = (r.u64()?, r.u64()?, r.u64()?, r.u64()?, r.u64()?);
    let replica = ReplicaId(r.u32()?);
    let (log, marks) = log_through(r, after, op)?;
    Ok(Message::DoViewChange(DoViewChange { view, after, log, marks, normal_view, op, commit, replica }))
}

/// A RECOVERYRESPONSE as a frame.
fn put_recovery_response(out: &mut Vec<u8>, epoch: u64, m: &RecoveryResponse) {
    put_u64s(out, RECOVERY_RESPONSE, epoch, &[m.view, m.nonce, m.began]);
    out.put_u32(m.replica.0);
    put_configuration(out, &m.configuration);
    out.put_u8(u8::from(m.log.is_some()));
    if let Some(taken) = &m.log {
        out.put_u64(taken.after);
        out.put_u64(taken.op);
        out.put_u64(taken.commit);
        put_log(out, &taken.log, &taken.marks);
    }
}

/// The rest of a RECOVERYRESPONSE, after its tag and epoch.
fn recovery_response(r: &mut Reader<'_>) -> Result<Message> {
    let (view, nonce, began, replica) = (r.u64()?, r.u64()?, r.u64()?, ReplicaId(r.u32()?));
    let configuration = read_configuration(r)?;
    let log = if flag(r)? {
        let (after, op, commit) = (r.u64()?, r.u64()?, r.u64()?);
        let (log, marks) = log_through(r, after, op)?;
        Some(PrimaryLog { after, log, marks, op, commit })
    } else {
        None
    };
    Ok(Message::RecoveryResponse(RecoveryResponse { view, nonce, replica, configuration, began, log }))
}

/// A flag: 0 or 1.
fn flag(r: &mut Reader<'_>) -> Result<bool> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(r.corrupt(&format!("{other} is no flag"))),
    }
}

/// A configuration, or none: a flag, then the configuration if the flag
/// is set.
fn put_optional(out: &mut Vec<u8>, mark: Option<&Mark>) {
    out.put_u8(u8::from(mark.is_some()));
    if let Some(mark) = mark {
        put_mark(out, mark);
    }
}

/// A mark, or none, as `put_optional` wrote it.
fn read_optional(r: &mut Reader<'_>) -> Result<Option<Mark>> {
    if flag(r)? { Ok(Some(read_mark(r)?)) } else { Ok(None) }
}

/// A tag, the epoch, then integers.
fn put_u64s(out: &mut Vec<u8>, tag: u8, epoch: u64, values: &[u64]) {
    out.put_u8(tag);
    out.put_u64(epoch);
    for value in values {
        out.put_u64(*value);
    }
}

/// A DOVIEWCHANGE: the tag, the epoch, its numbers, the replica, its log.
fn put_do_view_change(out: &mut Vec<u8>, epoch: u64, m: &DoViewChange) {
    put_u64s(out, DO_VIEW_CHANGE, epoch, &[m.view, m.after, m.normal_view, m.op, m.commit]);
    out.put_u32(m.replica.0);
    put_log(out, &m.log, &m.marks);
}

/// A log: its length, then each record; then its marks, how many, then
/// each position, the epoch it begins and its configuration.
fn put_log(out: &mut Vec<u8>, log: &[Vec<u8>], marks: &Marks) {
    out.put_u64(u64::try_from(log.len()).unwrap_or(u64::MAX));
    for record in log {
        out.put_bytes(record);
    }
    out.put_u32(u32::try_from(marks.len()).unwrap_or(u32::MAX));
    for (at, mark) in marks {
        out.put_u64(*at);
        put_mark(out, mark);
    }
}

/// A log of the records after `after` through `op`, refused if it holds
/// any other number, and its marks, refused if one is outside it. Each
/// record is read before the next is kept, so a damaged length cannot take
/// memory the frame does not hold.
fn log_through(r: &mut Reader<'_>, after: u64, op: u64) -> Result<(Vec<Vec<u8>>, Marks)> {
    let length = r.u64()?;
    if after.checked_add(length) != Some(op) {
        return Err(r.corrupt("its op-number is not its log's last position"));
    }
    let mut log = Vec::new();
    for _ in 0..length {
        log.push(r.bytes()?.to_vec());
    }
    let count = r.u32()?;
    let mut marks = Marks::new();
    for _ in 0..count {
        let at = r.u64()?;
        if at <= after || at > op {
            return Err(r.corrupt("a mark is outside its log"));
        }
        marks.insert(at, read_mark(r)?);
    }
    Ok((log, marks))
}

/// \[EXTENSION\] What VR keeps durable of its own, beside the records: its
/// epoch and the epoch's members [VRR §7], its view-number, the latest
/// view in which its status was normal (v′), and the commit-number it
/// knew, a lower bound of the one it knows made again (\[DESIGN\] a
/// departure: [VRR] keeps none, and a replica made again knows committed
/// only what its log let go; so made again, it delivers what it knew
/// committed, and takes the log of a replica in a later epoch, whose
/// mark it holds, as committed through the mark).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ViewState {
    /// The epoch.
    pub(crate) epoch: u64,
    /// The position of the mark the epoch began at.
    pub(crate) began: u64,
    /// The epoch's members.
    pub(crate) configuration: Configuration,
    /// The view-number.
    pub(crate) view: u64,
    /// The latest view in which its status was normal.
    pub(crate) normal_view: u64,
    /// The commit-number as the state was written: the records through
    /// it, written before, are committed, and never cut.
    pub(crate) commit: u64,
    /// Whether the replica was removed from the group: it serves nothing
    /// from then on, made again or not.
    pub(crate) removed: bool,
}

impl ViewState {
    /// As `PersistState` writes it.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_u64s(&mut out, STATE_FORMAT, self.epoch, &[self.began, self.view, self.normal_view, self.commit]);
        put_configuration(&mut out, &self.configuration);
        out.put_u8(u8::from(self.removed));
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
        let (epoch, began, view, normal_view, commit) = (r.u64()?, r.u64()?, r.u64()?, r.u64()?, r.u64()?);
        let configuration = read_configuration(&mut r)?;
        let removed = flag(&mut r)?;
        r.finish()?;
        if normal_view > view {
            return Err(r.corrupt("its normal view follows its view"));
        }
        Ok(ViewState { epoch, began, configuration, view, normal_view, commit, removed })
    }
}
