//! \[EXTENSION\] Viewstamped Replication as a [`Consensus`] (RULES.md D57),
//! from \[VRR\]: "VR uses a primary replica to order client requests; the
//! other replicas are backups that simply accept the order selected by the
//! primary" [VRR §3]. Its one client is the primary's database, whose
//! records are its requests; a record's op-number is its [`Lsn`].
//!
//! The state is [VRR §4]'s Figure 2: "the configuration", "the replica
//! number", "the current view-number, initially 0", "the current status",
//! "the op-number assigned to the most recently received request,
//! initially 0", "the log", "the commit-number" — but not "the
//! client-table": records are never resubmitted, so there is nothing to
//! filter (a departure, D57). "The primary is chosen round-robin, starting
//! with replica 1, as the system moves to new views": here the members in
//! order, the first being the primary of view 0.
//!
//! \[DESIGN\] Departures from \[VRR\], each for durability (D57: a record
//! durable before a backup acknowledges it, after \[VR88\]):
//! * [VRR §4.1] says "The protocol does not require any writing to disk";
//!   here a backup sends PREPAREOK only once the record is durable in its
//!   journal (`Persist`, then `Persisted`), and a primary counts itself
//!   toward a quorum only once its own copy is durable. So commit means a
//!   quorum holds the record durably, and the group survives losing power
//!   everywhere at once.
//! * The view-number is written (`PersistState`) before a replica sends
//!   STARTVIEWCHANGE or DOVIEWCHANGE, and with v′ and the new log before it
//!   takes part in a view as normal. [VRR §8.2] notes this is "the approach
//!   used in the original version of VR"; the STARTVIEWCHANGE round is
//!   kept all the same.
//! * A replica made again from its journal ([`Vr::restart`]) rejoins as
//!   [VRR §4.3] has one whose disk is intact do: "it is the same as a node
//!   that has been unable to communicate for some period of time". Its
//!   RECOVERY protocol is for a journal lost, and is a later stage. One that
//!   was the primary of its view does not serve in it again: records it
//!   sent may have been committed by its backups though its own copy did
//!   not survive, so it waits for the next view change; so too one that was
//!   in a view change, which it takes no further part in.
//! * The primary's database learns of commit through `Committed`, and does
//!   not wait to execute a request: it wrote the record before appending
//!   it. Backups' databases are delivered committed records to replay. A
//!   new primary serves only once the log it took over is committed and
//!   delivered, so its database has replayed every record before it writes
//!   its own; [VRR §4.2 step 4] has it accept requests at once.
//!
//! Normal operation is in `vr/normal.rs`, the view change in
//! `vr/view_change.rs`, state transfer in `vr/transfer.rs`, the frames in
//! `vr/frame.rs`.

mod frame;
mod normal;
mod transfer;
mod view_change;

use std::collections::{BTreeMap, BTreeSet};

use turborel::error::{Code, Result, SqlError};

use self::frame::{Message, ViewState};
use crate::{Consensus, Input, Lsn, Output, Recovered, ReplicaEvent, ReplicaId};

/// Its name, as [`Consensus::name`] gives it.
pub const NAME: &str = "vr";
/// \[DESIGN\] Ticks between a primary's heartbeats, by default: "if the
/// primary does not receive a new client request in a timely way, it
/// instead informs the backups of the latest commit by sending them a
/// ⟨COMMIT v, k⟩ message" [VRR §4.1 step 6].
pub const HEARTBEAT_TICKS: u64 = 2;
/// \[DESIGN\] Ticks a backup waits to hear from its primary, by default,
/// before it starts a view change: "If a timeout expires without a
/// communication from the primary, the replicas carry out a view change to
/// switch to a new primary" [VRR §4.2].
pub const VIEW_TIMEOUT_TICKS: u64 = 10;
/// \[DESIGN\] Records a primary sends again to a backup at each heartbeat,
/// past the last it acknowledged, by default: [VRR §4.1] leaves
/// "re-sending protocol messages that haven't received responses" to the
/// implementation.
pub const RESEND_RECORDS: u64 = 8;

/// \[EXTENSION\] What VR counts in ticks of its driver's timer (later, the
/// settings REPLICATION.HEARTBEAT_MS and REPLICATION.VIEW_TIMEOUT_MS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VrOptions {
    /// Ticks between a primary's heartbeats, and between messages of a view
    /// change or state transfer sent again.
    pub heartbeat_ticks: u64,
    /// Ticks without word from the primary, or in a view change or state
    /// transfer, before the next view change.
    pub view_timeout_ticks: u64,
    /// Records sent again to a backup at each heartbeat.
    pub resend_records: u64,
}

impl Default for VrOptions {
    fn default() -> Self {
        VrOptions { heartbeat_ticks: HEARTBEAT_TICKS, view_timeout_ticks: VIEW_TIMEOUT_TICKS, resend_records: RESEND_RECORDS }
    }
}

/// \[EXTENSION\] "The current status, either normal, view-change, or
/// recovering" [VRR §4, Figure 2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Normal: processing records.
    Normal,
    /// In a view change.
    ViewChange,
    /// \[EXTENSION\] Fetching the log of a later view it learned of, before
    /// it takes part in it [VRR §5.2]. \[VRR\]'s recovering status, for a
    /// journal lost, is a later stage.
    Transfer,
}

/// \[EXTENSION\] Where a replica stands, for the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    /// The view-number.
    pub view: u64,
    /// The status.
    pub status: Status,
    /// The primary of the view.
    pub primary: ReplicaId,
    /// The op-number: the position of its log's last record.
    pub op: Lsn,
    /// The commit-number, as far as it knows.
    pub commit: Lsn,
    /// Through where its journal holds its log durably.
    pub durable: Lsn,
    /// Through where its database said it applied the log.
    pub applied: Lsn,
}

/// Counts of journal writes, by which a `Persisted` is matched to the
/// `Persist` it answers: one each, in order.
#[derive(Debug, Default)]
struct Writes {
    /// `Persist`s given.
    given: u64,
    /// `Persisted`s taken.
    taken: u64,
    /// `Persist`s given before the last `PersistState`, which made them
    /// durable: their `Persisted` tell nothing new.
    synced: u64,
}

/// The ticks counted since things happened.
#[derive(Debug, Default)]
struct Timers {
    /// Since a backup last heard from its primary, or the status changed.
    heard: u64,
    /// Since the last heartbeat or message sent again.
    beat: u64,
}

/// What a primary keeps of its view.
#[derive(Debug, Default)]
struct Lead {
    /// Each backup's PREPAREOK with the highest op-number, in this view.
    acked: BTreeMap<ReplicaId, u64>,
    /// The op-number of the log it took over: records through it are
    /// delivered to its database before it serves.
    inherited: u64,
    /// Whether its database serves: `Role(Primary)` given.
    serving: bool,
}

/// A DOVIEWCHANGE's log and numbers.
#[derive(Debug, Clone)]
struct Candidate {
    log: Vec<Vec<u8>>,
    normal_view: u64,
    op: u64,
    commit: u64,
}

/// What a view change has gathered, for the view-number it is in.
#[derive(Debug, Default)]
struct Change {
    /// Whether this replica started it, and so may take part in it: one
    /// made again from its journal waits for the next (`Vr::restart`).
    open: bool,
    /// The replicas STARTVIEWCHANGE came from.
    started: BTreeSet<ReplicaId>,
    /// Whether this replica sent its DOVIEWCHANGE.
    done: bool,
    /// The new primary's: the DOVIEWCHANGEs, by replica.
    candidates: BTreeMap<ReplicaId, Candidate>,
}

/// \[EXTENSION\] Viewstamped Replication: one replica's state machine.
#[derive(Debug)]
pub struct Vr {
    members: Vec<ReplicaId>,
    me: ReplicaId,
    options: VrOptions,
    view: u64,
    /// v′: "the latest view in which its status was normal".
    normal_view: u64,
    status: Status,
    /// The log: the record at position n is at index n − 1.
    log: Vec<Vec<u8>>,
    commit: u64,
    /// Through where the journal holds the log durably.
    durable: u64,
    /// Through where `Deliver` has named.
    delivered: u64,
    applied: u64,
    writes: Writes,
    timers: Timers,
    lead: Lead,
    change: Change,
    /// The records this replica appended while serving, not yet committed.
    own: BTreeMap<u64, Vec<u8>>,
    /// Records appended in a service that ended before they were committed:
    /// in doubt until the commit-number passes them.
    in_doubt: BTreeMap<u64, Vec<u8>>,
    /// A GETSTATE asked and not yet answered, by the op-number it named.
    asked: Option<u64>,
}

impl Vr {
    /// \[EXTENSION\] A replica `me` of a new group of `members`, in view 0,
    /// its journal empty: once, when the group is made. A replica that ran
    /// before is made again by `restart`, even with its journal empty.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `members` is empty or names a replica twice, `me` is
    /// not one of them, or an option is 0.
    pub fn new(members: &[ReplicaId], me: ReplicaId, options: VrOptions) -> Result<Vr> {
        let mut sorted = members.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() != members.len() || !sorted.contains(&me) {
            return Err(misuse("the members name each replica once, this one among them"));
        }
        if options.heartbeat_ticks == 0 || options.view_timeout_ticks == 0 || options.resend_records == 0 {
            return Err(misuse("its options count at least one tick, and one record"));
        }
        Ok(Vr {
            members: sorted,
            me,
            options,
            view: 0,
            normal_view: 0,
            status: Status::Normal,
            log: Vec::new(),
            commit: 0,
            durable: 0,
            delivered: 0,
            applied: 0,
            writes: Writes::default(),
            timers: Timers::default(),
            lead: Lead::default(),
            change: Change::default(),
            own: BTreeMap::new(),
            in_doubt: BTreeMap::new(),
            asked: None,
        })
    }

    /// \[EXTENSION\] Replica `me` made again after a crash, from what its
    /// journal held: its records are durable, and it rejoins in the view it
    /// wrote last, normal there if it was normal there and was not its
    /// primary, else in a view change (see the module's documentation).
    /// Its commit-number is learned again from the group, and what is
    /// committed delivered again from the start: the driver gives the core
    /// only what it has not applied.
    ///
    /// # Errors
    ///
    /// As `new`; SQLCODE -916 if the state is malformed.
    pub fn restart(members: &[ReplicaId], me: ReplicaId, options: VrOptions, recovered: Recovered) -> Result<Vr> {
        let mut vr = Vr::new(members, me, options)?;
        let state = match recovered.state {
            Some(bytes) => ViewState::decode(&bytes)?,
            None => ViewState::default(),
        };
        vr.view = state.view;
        vr.normal_view = state.normal_view;
        vr.log = recovered.records;
        vr.durable = vr.op();
        if state.view != state.normal_view || vr.is_primary() {
            vr.status = Status::ViewChange;
        }
        Ok(vr)
    }

    /// \[EXTENSION\] Where this replica stands.
    pub fn standing(&self) -> Standing {
        Standing {
            view: self.view,
            status: self.status,
            primary: self.primary_of(self.view),
            op: Lsn(self.op()),
            commit: Lsn(self.commit),
            durable: Lsn(self.durable),
            applied: Lsn(self.applied),
        }
    }

    /// The op-number: its log's last position.
    fn op(&self) -> u64 {
        u64::try_from(self.log.len()).unwrap_or(u64::MAX)
    }

    /// The record at position `n`, if the log holds one.
    fn record(&self, n: u64) -> Option<&Vec<u8>> {
        let index = usize::try_from(n.checked_sub(1)?).ok()?;
        self.log.get(index)
    }

    /// "The primary is chosen round-robin, starting with replica 1, as the
    /// system moves to new views" [VRR §4].
    fn primary_of(&self, view: u64) -> ReplicaId {
        let count = u64::try_from(self.members.len()).unwrap_or(u64::MAX);
        let index = view.checked_rem(count).and_then(|i| usize::try_from(i).ok()).unwrap_or(0);
        self.members.get(index).copied().unwrap_or(self.me)
    }

    /// Whether this replica is the primary of its view.
    fn is_primary(&self) -> bool {
        self.primary_of(self.view) == self.me
    }

    /// "A group of f + 1 replicas is often referred to as a quorum"; for a
    /// group of another size K, "the threshold is the largest f such that
    /// 2f + 1 is less than or equal to the group size, K, and a quorum is
    /// of size K − f" [VRR §2.2].
    fn quorum(&self) -> usize {
        let threshold = self.members.len().saturating_sub(1).checked_div(2).unwrap_or(0);
        self.members.len().saturating_sub(threshold)
    }

    /// The other members.
    fn others(&self) -> Vec<ReplicaId> {
        self.members.iter().copied().filter(|&r| r != self.me).collect()
    }

    /// Sends `message` to `to`.
    fn send(to: ReplicaId, message: &Message, out: &mut Vec<Output>) {
        out.push(Output::Send { to, frame: message.encode() });
    }

    /// Sends `message` to every other member.
    fn broadcast(&self, message: &Message, out: &mut Vec<Output>) {
        let frame = message.encode();
        for to in self.others() {
            out.push(Output::Send { to, frame: frame.clone() });
        }
    }

    /// Writes `record` at `at` to the journal.
    fn persist(&mut self, at: u64, record: Vec<u8>, out: &mut Vec<Output>) {
        out.push(Output::Persist { at: Lsn(at), record });
        self.writes.given = self.writes.given.saturating_add(1);
    }

    /// Writes the view-number and v′, which makes the journal durable
    /// through the log's end.
    fn persist_state(&mut self, out: &mut Vec<Output>) {
        let state = ViewState { view: self.view, normal_view: self.normal_view };
        out.push(Output::PersistState(state.encode()));
        self.writes.synced = self.writes.given;
        self.durable = self.op();
    }

    /// Takes a frame from `from`, refusing it if `from` is no other member
    /// or the frame is malformed.
    fn receive(&mut self, from: ReplicaId, frame: &[u8], out: &mut Vec<Output>) {
        if from == self.me || !self.members.contains(&from) {
            return refuse(from, "[EXTENSION] RULES.md D57: a frame from no other member of the group", out);
        }
        match Message::decode(frame) {
            Ok(message) => self.dispatch(from, message, out),
            Err(e) => refuse(from, &e.described(), out),
        }
    }

    /// Takes a message from `from`. A message naming a replica that did not
    /// send it, or a primary's message from no primary of its view, is
    /// refused: the transport says who sent each frame.
    fn dispatch(&mut self, from: ReplicaId, message: Message, out: &mut Vec<Output>) {
        let sender = match &message {
            Message::Prepare(m) => self.primary_of(m.view),
            Message::Commit(m) => self.primary_of(m.view),
            Message::StartView(m) => self.primary_of(m.view),
            Message::PrepareOk(m) => m.replica,
            Message::StartViewChange(m) => m.replica,
            Message::DoViewChange(m) => m.replica,
            Message::GetState(m) => m.replica,
            Message::NewState(_) => from,
        };
        if sender != from {
            return refuse(from, "[EXTENSION] RULES.md D57: a message names a sender other than the replica it came from", out);
        }
        match message {
            Message::Prepare(m) => self.on_prepare(m, out),
            Message::PrepareOk(m) => self.on_prepare_ok(m, out),
            Message::Commit(m) => self.on_commit(m, out),
            Message::StartViewChange(m) => self.on_start_view_change(m, out),
            Message::DoViewChange(m) => self.on_do_view_change(m, out),
            Message::StartView(m) => self.on_start_view(m, out),
            Message::GetState(m) => self.on_get_state(m, out),
            Message::NewState(m) => self.on_new_state(m, out),
        }
    }
}

impl Consensus for Vr {
    fn name(&self) -> &'static str {
        NAME
    }

    fn step(&mut self, input: Input, out: &mut Vec<Output>) {
        match input {
            Input::Append(record) => self.append(record, out),
            Input::Received { from, frame } => self.receive(from, &frame, out),
            Input::Tick => self.tick(out),
            Input::Persisted(at) => self.persisted(at.0, out),
            Input::Applied(at) => self.applied = self.applied.max(at.0),
            // VR keeps its whole log (log retention is a later stage), so it
            // never asks for a state transfer, and none is installed.
            Input::Installed(_) => {}
        }
        self.serve(out);
    }
}

/// An error of the host's use of the API (SQLCODE -910).
fn misuse(why: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D57: {why}"))
}

/// Reports a frame refused.
fn refuse(from: ReplicaId, why: &str, out: &mut Vec<Output>) {
    out.push(Output::Event(ReplicaEvent::FrameRefused { from, why: why.to_owned() }));
}
