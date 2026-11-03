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
//!   that has been unable to communicate for some period of time". One
//!   whose journal holds no state, lost or never written, recovers from
//!   the group first by \[VRR\]'s RECOVERY protocol (`vr/recovery.rs`).
//!   One that was the primary of its view does not serve in it again:
//!   records it sent may have been committed by its backups though its
//!   own copy did not survive, so it waits for the next view change; so
//!   too one that was in a view change, which it takes no further part in.
//! * The primary's database learns of commit through `Committed`, and does
//!   not wait to execute a request: it wrote the record before appending
//!   it. Backups' databases are delivered committed records to replay. A
//!   new primary serves only once the log it took over is committed and
//!   delivered, so its database has replayed every record before it writes
//!   its own; [VRR §4.2 step 4] has it accept requests at once.
//!
//! Normal operation is in `vr/normal.rs`, the view change in
//! `vr/view_change.rs`, state transfer in `vr/transfer.rs`, recovery in
//! `vr/recovery.rs`, reconfiguration in `vr/reconfigure.rs`, the frames
//! in `vr/frame.rs`.

mod frame;
mod normal;
mod reconfigure;
mod recovery;
mod standing;
mod transfer;
mod view_change;
mod window;

use std::collections::{BTreeMap, BTreeSet};

use turborel::error::{Code, Result, SqlError};

use self::frame::{Marks, Message, ViewState};
use self::recovery::Recovering;
pub use self::standing::Standing;
use self::window::Window;
use crate::{Configuration, Consensus, Input, Lsn, Member, Output, Recovered, ReplicaEvent, ReplicaId};

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
    /// it takes part in it [VRR §5.2]; or a state transfer of its database,
    /// the log alone no longer reaching back to what it holds (RULES.md
    /// D57).
    Transfer,
    /// Recovering: its journal held no state, so it learns the group's
    /// from the others before it takes part [VRR §4.3] (`vr/recovery.rs`).
    Recovering,
    /// \[EXTENSION\] Leaving: no member of the epoch entered, it answers
    /// the new members until enough of them are normal in it, then is
    /// removed [VRR §7] (`vr/reconfigure.rs`).
    Leaving,
    /// \[EXTENSION\] Removed: it takes no further part.
    Removed,
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

/// A DOVIEWCHANGE's log, a window after `after`, its marks, and numbers.
#[derive(Debug, Clone, Default)]
struct Candidate {
    after: u64,
    log: Vec<Vec<u8>>,
    marks: Marks,
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
    /// The epoch [VRR §7].
    epoch: u64,
    /// The epoch's members, with their addresses.
    configuration: Configuration,
    /// The members' numbers, in order: the primary of view v is the
    /// (v mod n)th.
    members: Vec<ReplicaId>,
    me: ReplicaId,
    options: VrOptions,
    view: u64,
    /// v′: "the latest view in which its status was normal".
    normal_view: u64,
    status: Status,
    /// The log: a window, the records through its first position let go by
    /// retention (`window.rs`).
    log: Window,
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
    /// Deposed, its database opening again: it serves no view until the
    /// database tells where it holds the log (`opened`).
    reopening: bool,
    /// What its recovery has gathered, while recovering.
    recovering: Recovering,
    /// Whether its view-number is not yet in its journal: a replica of a
    /// new group writes it as it is opened, so a journal holding records
    /// holds the state too.
    unwritten: bool,
    /// The position of the mark the epoch began at: 0 in the first.
    epoch_began: u64,
    /// Leaving, the new epoch's members heard normal in it.
    started: BTreeSet<ReplicaId>,
    /// In a transfer, the replica the state and the log are fetched from,
    /// when it is not the primary of the view: the sender of a message of
    /// a later epoch, or of a NEEDSTATE (`vr/transfer.rs`).
    source: Option<ReplicaId>,
}

impl Vr {
    /// \[EXTENSION\] A replica `me` of a new group of `configuration`, in
    /// epoch 0, view 0, its journal empty, by fiat: where the host knows
    /// the group is new, as a test harness does. A node's replica is made
    /// by `restart` from its journal, which, empty, has it recover from
    /// the group.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `configuration` is empty or names a replica twice,
    /// `me` is not one of them, or an option is 0.
    pub fn new(configuration: &[Member], me: ReplicaId, options: VrOptions) -> Result<Vr> {
        let sorted = members_of(configuration);
        if sorted.len() != configuration.len() || !sorted.contains(&me) {
            return Err(misuse("the members name each replica once, this one among them"));
        }
        if options.heartbeat_ticks == 0 || options.view_timeout_ticks == 0 || options.resend_records == 0 {
            return Err(misuse("its options count at least one tick, and one record"));
        }
        Ok(Vr {
            epoch: 0,
            configuration: configuration.to_vec(),
            members: sorted,
            me,
            options,
            view: 0,
            normal_view: 0,
            status: Status::Normal,
            log: Window::default(),
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
            reopening: false,
            recovering: Recovering::default(),
            unwritten: true,
            epoch_began: 0,
            started: BTreeSet::new(),
            source: None,
        })
    }

    /// \[EXTENSION\] Replica `me` made again after a crash, from what its
    /// journal held: its records are durable, and it rejoins in the view it
    /// wrote last, normal there if it was normal there and was not its
    /// primary, else in a view change (see the module's documentation).
    /// Its commit-number is learned again from the group; its driver tells
    /// where its database, opened from its files, holds the log
    /// (`Input::Opened`), and what is committed after it is delivered. A
    /// journal holding no state, lost or never written, makes a replica
    /// recovering (`vr/recovery.rs`), its RECOVERY carrying `nonce`, a
    /// number no earlier start of it had. The epoch and configuration
    /// the state holds are its, over `configuration`, the node's setting,
    /// which is its starting point only [VRR §7].
    ///
    /// # Errors
    ///
    /// As `new`; SQLCODE -916 if the state is malformed.
    pub fn restart(configuration: &[Member], me: ReplicaId, options: VrOptions, recovered: Recovered, nonce: u64) -> Result<Vr> {
        let mut vr = Vr::new(configuration, me, options)?;
        let marks = recovered.marks.iter().map(|(at, c)| (at.0, c.clone())).collect();
        vr.log = Window::new(recovered.first.0, recovered.records, marks);
        // Every record let go was committed and applied: its database holds
        // it in its files.
        vr.commit = recovered.first.0;
        vr.delivered = recovered.first.0;
        vr.applied = recovered.first.0;
        vr.durable = vr.op();
        let Some(bytes) = recovered.state else {
            vr.status = Status::Recovering;
            vr.recovering.nonce = nonce;
            return Ok(vr);
        };
        let state = ViewState::decode(&bytes)?;
        vr.epoch = state.epoch;
        vr.epoch_began = state.began;
        if !state.configuration.is_empty() {
            vr.members = members_of(&state.configuration);
            vr.configuration = state.configuration;
        }
        vr.view = state.view;
        vr.normal_view = state.normal_view;
        // What it knew committed as the state was written: the records
        // through it, written before, are durable with the state, and were
        // never cut since.
        vr.commit = vr.commit.max(state.commit.min(vr.op()));
        vr.unwritten = false;
        if state.removed {
            vr.status = Status::Removed;
        } else if !vr.members.contains(&vr.me) {
            vr.status = Status::Leaving;
        } else if vr.behind_epoch() {
            // The epoch adopted from behind, its commit-number before the
            // mark the epoch began at: it fetches the state first
            // (`vr/reconfigure.rs`).
            vr.status = Status::Transfer;
        } else if state.view != state.normal_view || vr.is_primary() {
            vr.status = Status::ViewChange;
        }
        Ok(vr)
    }

    /// The op-number: its log's last position.
    fn op(&self) -> u64 {
        self.log.op()
    }

    /// The record at position `n`, if the log holds one.
    fn record(&self, n: u64) -> Option<&Vec<u8>> {
        self.log.record(n)
    }

    /// \[EXTENSION\] `Input::Retain`: the records through `through` let go,
    /// as far as they are committed (RULES.md D57); the journal told.
    fn retain(&mut self, through: u64, out: &mut Vec<Output>) {
        let through = through.min(self.commit);
        if through <= self.log.first() {
            return;
        }
        self.log.retain_after(through);
        out.push(Output::Retain { through: Lsn(through) });
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

    /// Sends `message` to `to`, in this epoch.
    fn send(&self, to: ReplicaId, message: &Message, out: &mut Vec<Output>) {
        out.push(Output::Send { to, frame: message.encode(self.epoch) });
    }

    /// Sends `message` to every other member.
    fn broadcast(&self, message: &Message, out: &mut Vec<Output>) {
        let frame = message.encode(self.epoch);
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
        let state = ViewState {
            epoch: self.epoch,
            began: self.epoch_began,
            configuration: self.configuration.clone(),
            view: self.view,
            normal_view: self.normal_view,
            commit: self.commit,
            removed: self.status == Status::Removed,
        };
        out.push(Output::PersistState(state.encode()));
        self.writes.synced = self.writes.given;
        self.durable = self.op();
        self.unwritten = false;
    }

    /// Takes a frame from `from`, refusing it if it is malformed or from
    /// this replica. One from no member of the epoch is taken as
    /// `taken_outside` says (`vr/reconfigure.rs`); one of an earlier epoch
    /// from a member behind it is answered EPOCHSTARTED, by which the
    /// member asks for the state, but a RECOVERY, which a new member
    /// sends knowing no epoch, and a GETSTATE, answered so the asker
    /// learns the epoch; one of a later epoch has this replica learn it.
    /// A removed replica takes nothing.
    fn receive(&mut self, from: ReplicaId, frame: &[u8], out: &mut Vec<Output>) {
        let (epoch, message) = match Message::decode(frame) {
            Ok(decoded) => decoded,
            Err(e) => return refuse(from, &e.described(), out),
        };
        if from == self.me {
            let why = format!(
                "[EXTENSION] RULES.md D57: a {} of epoch {epoch} from this replica itself, {:?} in view {} of epoch {}",
                message.kind(),
                self.status,
                self.view,
                self.epoch
            );
            return refuse(from, &why, out);
        }
        if !self.members.contains(&from) && self.status != Status::Leaving {
            return self.taken_outside(from, (epoch, message), out);
        }
        if self.status == Status::Removed {
            return;
        }
        let of_any_epoch = matches!(message, Message::Recovery(_) | Message::GetState(_) | Message::RecoveryResponse(_));
        if epoch < self.epoch && !of_any_epoch {
            // A member behind the epoch: told it started, by which it asks
            // for the state and learns it.
            return self.tell_left(from, out);
        }
        if epoch > self.epoch && !of_any_epoch {
            return self.later_epoch(epoch, from, &message, out);
        }
        self.dispatch(from, (epoch, message), out);
    }

    /// Takes a message from `from`, sent in `epoch`. A message naming a
    /// replica that did not send it, or a primary's message from no
    /// primary of its view, is refused: the transport says who sent each
    /// frame. A replica recovering takes only the messages of its
    /// recovery; one leaving, what the new members ask of it.
    fn dispatch(&mut self, from: ReplicaId, (epoch, message): (u64, Message), out: &mut Vec<Output>) {
        let sender = match &message {
            Message::Prepare(m) => self.primary_of(m.view),
            Message::Commit(m) => self.primary_of(m.view),
            Message::StartView(m) => self.primary_of(m.view),
            Message::PrepareOk(m) => m.replica,
            Message::StartViewChange(m) => m.replica,
            Message::DoViewChange(m) => m.replica,
            Message::GetState(m) => m.replica,
            Message::Recovery(m) => m.replica,
            Message::RecoveryResponse(m) => m.replica,
            Message::EpochStarted(m) => m.replica,
            Message::NewState(_) | Message::NeedState(_) => from,
        };
        let of_recovery = matches!(message, Message::Recovery(_) | Message::RecoveryResponse(_));
        if self.status == Status::Recovering && !of_recovery {
            // Recovering, its members its node's setting only: it judges
            // no sender by them, and takes nothing but its recovery's.
            return;
        }
        if sender != from {
            let why = format!(
                "[EXTENSION] RULES.md D57: a {} of epoch {epoch} naming replica {} came from replica {}, {:?} in view {} of epoch {}",
                message.kind(),
                sender.0,
                from.0,
                self.status,
                self.view,
                self.epoch
            );
            return refuse(from, &why, out);
        }
        let asked_of_leaving = matches!(message, Message::Recovery(_) | Message::GetState(_) | Message::EpochStarted(_));
        if self.status == Status::Leaving && !asked_of_leaving {
            return;
        }
        match message {
            Message::Prepare(m) => self.on_prepare(m, out),
            Message::PrepareOk(m) => self.on_prepare_ok(m, out),
            Message::Commit(m) => self.on_commit(m, out),
            Message::StartViewChange(m) => self.on_start_view_change(m, out),
            Message::DoViewChange(m) => self.on_do_view_change(m, out),
            Message::StartView(m) => self.on_start_view(m, out),
            Message::GetState(m) => self.on_get_state(m, epoch, out),
            Message::NewState(m) => self.on_new_state(m, out),
            Message::NeedState(m) => self.on_need_state(from, &m, out),
            Message::Recovery(m) => self.on_recovery(m, out),
            Message::RecoveryResponse(m) => self.on_recovery_response(m, epoch, out),
            Message::EpochStarted(m) => self.on_epoch_started(&m, out),
        }
    }
}

/// The members of `configuration`, by number, each once.
fn members_of(configuration: &[Member]) -> Vec<ReplicaId> {
    let mut members: Vec<ReplicaId> = configuration.iter().map(|m| m.id).collect();
    members.sort_unstable();
    members.dedup();
    members
}

impl Consensus for Vr {
    fn name(&self) -> &'static str {
        NAME
    }

    fn describe(&self) -> String {
        self.described()
    }

    fn step(&mut self, input: Input, out: &mut Vec<Output>) {
        match input {
            Input::Append(record) => self.append(record, out),
            Input::Received { from, frame } => self.receive(from, &frame, out),
            Input::Tick => self.tick(out),
            Input::Persisted(at) => self.persisted(at.0, out),
            Input::Applied(at) => self.applied = self.applied.max(at.0),
            Input::Opened(at) => self.opened(at.0, out),
            Input::Retain(through) => self.retain(through.0, out),
            Input::Installed { holds, records } => self.installed(holds.0, records, out),
            Input::Reconfigure { record, configuration } => self.reconfigure(record, configuration, out),
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
