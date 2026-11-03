//! \[EXTENSION\] VR's reconfiguration [VRR §7] (RULES.md D57): the group's
//! members changed while it runs, in epochs. The primary, asked by its
//! database (`Input::Reconfigure`), appends the database's record of it
//! as the last of the epoch, marked with the next epoch's members, and
//! takes no record after it, serving on until the mark is committed, so
//! its database learns the commit. Once the mark is committed, every replica
//! that learns so enters the next epoch: view 0, its members the mark's;
//! a member of it is normal there, the first of them its primary, which
//! serves once the log it took over is committed and delivered; a
//! replica that is no member is leaving: it answers the new members'
//! GETSTATE and RECOVERY, and the state transfers they fetch, until it
//! hears EPOCHSTARTED from a quorum of them, normal in the epoch, and
//! is then removed, its database retired.
//!
//! \[DESIGN\] Departures: new members join by the recovery protocol
//! (`recovery.rs`), not STARTEPOCH: started with their node's settings
//! naming the new members, they recover from those normal in the epoch,
//! adopting its configuration from the response; the old primary's
//! database takes no change after the reconfiguration, refused (SQLCODE
//! -917), and is deposed as the epoch is entered, until the next epoch's
//! primary serves; a replica behind an epoch, told by a message of a later one,
//! fetches the log from the sender, whose NEWSTATE carries the epoch's
//! configuration, and enters the epoch as it learns the mark committed.

use super::frame::{EpochStarted, Message};
use super::{Change, Lead, Status, Vr, refuse};
use crate::{Configuration, Lsn, Mark, Output, ReplicaEvent, ReplicaId};

impl Vr {
    /// `Input::Reconfigure`: the primary, serving, appends `record` marked
    /// a reconfiguration to `configuration`, as the epoch's last, and
    /// nothing more; it serves on until the mark is committed, so its
    /// database learns the commit, then is deposed as the epoch is
    /// entered.
    pub(super) fn reconfigure(&mut self, record: Vec<u8>, configuration: Configuration, out: &mut Vec<Output>) {
        if !self.lead.serving || self.reconfiguring() || configuration.is_empty() {
            out.push(Output::Event(ReplicaEvent::AppendRefused));
            return;
        }
        self.append_marked(record, Some(configuration), out);
    }

    /// Whether this replica is behind its epoch: its commit-number before
    /// the mark the epoch began at, the epoch adopted from behind
    /// (`adopt_behind`), so its log, whatever it holds at the mark's
    /// position, is of the epoch before, and it takes part in no view
    /// change of the epoch, nor answers for its log, until it fetched the
    /// state of a member; a replica that entered the epoch had the mark
    /// committed.
    pub(super) fn behind_epoch(&self) -> bool {
        self.commit < self.epoch_began
    }

    /// Whether the log holds a reconfiguration not yet entered: no record
    /// is appended after it, and no primary serves, until its epoch is
    /// entered; a mark let go with the log by a view change, uncommitted,
    /// holds nothing back.
    pub(super) fn reconfiguring(&self) -> bool {
        self.log.next_mark(self.entered_through()).is_some()
    }

    /// A frame from `from`, no member of this epoch, this replica not
    /// leaving it. Recovering, its members its node's setting only, this
    /// replica takes the RECOVERYRESPONSE of whoever answers. A RECOVERY
    /// is a new member's, of an epoch this replica has not entered: it
    /// waits, sent again at the member's heartbeats. A frame of a later
    /// epoch has this replica learn it. Of an earlier epoch, a GETSTATE
    /// is answered with the state, by which the asker learns the epoch,
    /// and another frame with EPOCHSTARTED, by which the sender asks. Of
    /// this epoch, a GETSTATE, a replica leaving's, is answered
    /// EPOCHSTARTED, by which it counts the members normal in the epoch,
    /// a NEWSTATE or NEEDSTATE, an answer come late from a replica that
    /// left, is dropped, and another frame is refused. An answer to a
    /// replica the transport does not link, one this replica was never in
    /// a group with, is dropped by the driver; a member that was answers.
    pub(super) fn taken_outside(&mut self, from: ReplicaId, (epoch, message): (u64, Message), out: &mut Vec<Output>) {
        if self.status == Status::Recovering {
            if let Message::RecoveryResponse(m) = message
                && m.replica == from
            {
                self.on_recovery_response(m, epoch, out);
            }
            return;
        }
        if matches!(message, Message::Recovery(_)) {
            return;
        }
        if epoch > self.epoch {
            return self.later_epoch(epoch, from, &message, out);
        }
        match message {
            Message::GetState(m) if m.replica == from && epoch < self.epoch => self.on_get_state(m, epoch, out),
            Message::GetState(m) if m.replica == from => self.tell_left(from, out),
            // The answer of the replica this one asked for the state, one
            // leaving the epoch, which answers as a member does.
            Message::NewState(m) if self.source == Some(from) => self.on_new_state(m, out),
            Message::NeedState(m) if self.source == Some(from) => self.on_need_state(from, &m, out),
            // An answer to a GETSTATE or RECOVERY this replica sent before
            // it entered the epoch, come late from a replica that left it;
            // or its EPOCHSTARTED, a nudge come late, which a member needs
            // not.
            Message::NewState(_) | Message::NeedState(_) | Message::RecoveryResponse(_) | Message::EpochStarted(_) => {}
            Message::GetState(_)
            | Message::Prepare(_)
            | Message::PrepareOk(_)
            | Message::Commit(_)
            | Message::StartViewChange(_)
            | Message::DoViewChange(_)
            | Message::StartView(_)
                if epoch < self.epoch =>
            {
                self.tell_left(from, out);
            }
            Message::GetState(_)
            | Message::Prepare(_)
            | Message::PrepareOk(_)
            | Message::Commit(_)
            | Message::StartViewChange(_)
            | Message::DoViewChange(_)
            | Message::StartView(_)
            | Message::Recovery(_) => {
                let why = format!(
                    "[EXTENSION] RULES.md D57: a {} of epoch {epoch} from replica {}, no member of epoch {}, which this replica is {:?} in",
                    message.kind(),
                    from.0,
                    self.epoch,
                    self.status
                );
                refuse(from, &why, out);
            }
        }
    }

    /// The commit-number having advanced: each mark committed and not
    /// yet entered enters its epoch, in order; one of an epoch not after
    /// this replica's, a mark of an epoch it passed by a state transfer,
    /// is passed.
    pub(super) fn check_epoch(&mut self, out: &mut Vec<Output>) {
        loop {
            let Some((at, mark)) = self.log.next_mark(self.entered_through()) else { return };
            if at > self.commit {
                return;
            }
            let mark = mark.clone();
            if mark.epoch <= self.epoch {
                self.epoch_began = at;
                continue;
            }
            self.enter_epoch(at, mark, out);
        }
    }

    /// The position of the last mark entered: the epoch's first position
    /// less one, as every mark before the log's first position was entered
    /// before the log was retained there.
    fn entered_through(&self) -> u64 {
        self.epoch_began.max(self.log.first())
    }

    /// Enters the epoch `mark`, marked at `at`, begins: see the module's
    /// documentation. A replica leaving the epoch before, learning this
    /// one, is removed: the epoch it was leaving started and moved on.
    fn enter_epoch(&mut self, at: u64, mark: Mark, out: &mut Vec<Output>) {
        let was_leaving = self.status == Status::Leaving;
        let Mark { epoch, configuration } = mark;
        self.depose(out);
        // The mark is the last record of the epoch before: records after
        // it, held while the mark was not known committed, were appended
        // in that epoch, by a primary behind the reconfiguration, and never
        // committed; the epoch's primary appends at those positions. Known
        // committed past the mark, from a log of the epoch, they are the
        // epoch's, kept.
        if self.op() > at && self.commit <= at {
            out.push(Output::Truncate { after: Lsn(at) });
            self.log.truncate_to(at);
            self.durable = self.durable.min(at);
        }
        self.epoch = epoch;
        self.epoch_began = at;
        let before = std::mem::replace(&mut self.members, super::members_of(&configuration));
        let leaving: Vec<ReplicaId> = before.into_iter().filter(|id| !self.members.contains(id)).collect();
        self.configuration.clone_from(&configuration);
        self.view = 0;
        self.normal_view = 0;
        self.lead = Lead { inherited: self.op(), ..Lead::default() };
        self.change = Change::default();
        self.timers.heard = 0;
        self.asked = None;
        self.source = None;
        self.started.clear();
        if self.members.contains(&self.me) {
            self.status = Status::Normal;
        } else if was_leaving {
            self.status = Status::Removed;
        } else {
            self.status = Status::Leaving;
        }
        self.persist_state(out);
        out.push(Output::Epoch { epoch: self.epoch, configuration: configuration.clone() });
        out.push(Output::Event(ReplicaEvent::Reconfigured { epoch: self.epoch, configuration }));
        if self.status == Status::Removed {
            out.push(Output::Retire);
            out.push(Output::Event(ReplicaEvent::Removed));
        }
        if self.status == Status::Normal {
            out.push(Output::Event(ReplicaEvent::ViewChanged { view: 0, primary: self.primary_of(0) }));
            self.tell_started(&leaving, out);
            // The epoch's primary serves once it delivered the log it took
            // over, which, serving before, it did not.
            self.deliver(out);
        }
    }

    /// Adopts the epoch `mark` begins, at `began`, from behind: this
    /// replica's log does not reach the mark, so it is in a transfer,
    /// taking part in no view change of the epoch, until it fetched the
    /// state of a member, which `Vr::restart` sees by the commit-number
    /// its state keeps, before the mark [VRR §7]; no member of the epoch,
    /// it is leaving it, needing no state.
    fn adopt_behind(&mut self, began: u64, mark: Mark, out: &mut Vec<Output>) {
        if mark.epoch <= self.epoch {
            return;
        }
        let was_leaving = self.status == Status::Leaving;
        let Mark { epoch, configuration } = mark;
        self.epoch = epoch;
        self.epoch_began = began;
        self.members = super::members_of(&configuration);
        self.configuration.clone_from(&configuration);
        self.view = 0;
        self.normal_view = 0;
        self.started.clear();
        if !self.members.contains(&self.me) {
            // Leaving an epoch before: the epoch after it started, and
            // moved on, so this replica is needed no more.
            self.status = if was_leaving { Status::Removed } else { Status::Leaving };
            self.asked = None;
        }
        self.persist_state(out);
        out.push(Output::Epoch { epoch, configuration: configuration.clone() });
        out.push(Output::Event(ReplicaEvent::Reconfigured { epoch, configuration }));
        if self.status == Status::Removed {
            out.push(Output::Retire);
            out.push(Output::Event(ReplicaEvent::Removed));
        }
    }

    /// EPOCHSTARTED to each of `leaving`, the members of the epoch before
    /// that are no members of this one.
    fn tell_started(&self, leaving: &[ReplicaId], out: &mut Vec<Output>) {
        let started = Message::EpochStarted(EpochStarted { replica: self.me, configuration: self.configuration.clone() });
        for &to in leaving.iter().filter(|&&id| id != self.me) {
            self.send(to, &started, out);
        }
    }

    /// Leaving, asks the epoch's members whether they are normal in it, by
    /// a GETSTATE, which one normal answers with EPOCHSTARTED too: so a
    /// replica that missed EPOCHSTARTED is removed all the same; one in
    /// a later epoch answers with its state, by which this replica
    /// learns the epoch after the one it is leaving, and is removed.
    pub(super) fn ask_started(&mut self, out: &mut Vec<Output>) {
        self.asked = Some(self.commit);
        let get = Message::GetState(super::frame::GetState { view: self.view, op: self.commit, replica: self.me });
        for &to in self.members.iter().filter(|&&id| id != self.me) {
            self.send(to, &get, out);
        }
    }

    /// A frame from a replica behind this epoch, a member of it or of the
    /// epoch before, or leaving this one: answered EPOCHSTARTED, once this
    /// replica is in the epoch, normal or leaving, by which one behind the
    /// epoch asks for the state, and one leaving counts the members normal
    /// in it, among which a replica leaving is not.
    pub(super) fn tell_left(&self, from: ReplicaId, out: &mut Vec<Output>) {
        if matches!(self.status, Status::Normal | Status::Leaving) {
            let started = EpochStarted { replica: self.me, configuration: self.configuration.clone() };
            self.send(from, &Message::EpochStarted(started), out);
        }
    }

    /// An EPOCHSTARTED from a member of the epoch: leaving, this replica
    /// is removed once a quorum of them are normal in it.
    pub(super) fn on_epoch_started(&mut self, m: &EpochStarted, out: &mut Vec<Output>) {
        if self.status != Status::Leaving || !self.members.contains(&m.replica) {
            return;
        }
        self.started.insert(m.replica);
        if self.started.len() >= self.quorum() {
            self.status = Status::Removed;
            self.persist_state(out);
            out.push(Output::Retire);
            out.push(Output::Event(ReplicaEvent::Removed));
        }
    }

    /// A message of `epoch`, later than this replica's, from `from`: the
    /// log is fetched from the sender, whose NEWSTATE carries the epoch's
    /// configuration, or, the sender being a member this replica cannot
    /// reach, no member of its epoch and not an EPOCHSTARTED's, which
    /// carries the members to reach, from the primary of its view, or
    /// every other member if that is this replica, those in the later
    /// epoch or leaving it answering; a NEEDSTATE answering has the
    /// sender's state fetched; the epoch is entered as the mark is
    /// learned committed. A replica serving is deposed first; one asking
    /// already asks no more.
    pub(super) fn later_epoch(&mut self, epoch: u64, from: ReplicaId, message: &Message, out: &mut Vec<Output>) {
        if self.status == Status::Recovering {
            return;
        }
        match message {
            Message::NewState(m) => return self.take_epoch_state(epoch, m.clone(), out),
            Message::NeedState(m) => {
                // Behind the sender's window: its epoch is adopted first,
                // so a crash meanwhile leaves this replica behind in the
                // epoch, not in the one before; then its state is
                // fetched, and the log after it from it.
                if matches!(self.status, Status::Transfer | Status::Leaving) && self.asked == Some(m.op) {
                    self.adopt_behind(m.began, Mark { epoch, configuration: m.configuration.clone() }, out);
                    if self.status == Status::Transfer {
                        self.fetch_state_from(from, out);
                    }
                }
                return;
            }
            Message::EpochStarted(m) => {
                // The sender may be new to the group, unknown to this
                // replica's transport: the epoch's members are linked.
                out.push(Output::Reach(m.configuration.clone()));
            }
            Message::Prepare(_)
            | Message::PrepareOk(_)
            | Message::Commit(_)
            | Message::StartViewChange(_)
            | Message::DoViewChange(_)
            | Message::StartView(_)
            | Message::GetState(_)
            | Message::Recovery(_)
            | Message::RecoveryResponse(_) => {}
        }
        if matches!(self.status, Status::Transfer | Status::Leaving) && self.asked == Some(self.commit) {
            return;
        }
        let reachable = self.members.contains(&from) || matches!(message, Message::EpochStarted(_));
        if self.status == Status::Leaving {
            // Leaving still: the state asked for, by which this replica
            // learns the epoch and is removed.
            self.source = reachable.then_some(from);
            self.get_state(self.commit, out);
            return;
        }
        self.depose(out);
        self.status = Status::Transfer;
        self.lead = Lead::default();
        self.change = Change::default();
        self.timers.heard = 0;
        self.asked = None;
        self.source = reachable.then_some(from);
        self.get_state(self.commit, out);
    }

    /// A NEWSTATE of a later epoch, answering this replica's GETSTATE, in
    /// a transfer or leaving: the log after its commit-number replaced by
    /// the sender's, the marks among them kept, the sender's commit-number
    /// learned, which is past the mark the sender's epoch began at; the
    /// epochs are entered through the marks, as `check_epoch` finds them
    /// committed, or, the sender's mark before the log fetched, by its
    /// configuration, told in the message.
    fn take_epoch_state(&mut self, epoch: u64, m: super::frame::NewState, out: &mut Vec<Output>) {
        if !matches!(self.status, Status::Transfer | Status::Leaving) || m.after > self.commit || m.after < self.log.first() {
            return;
        }
        if !self.install(m.after, (m.log, m.marks), out) {
            return;
        }
        self.asked = None;
        // The sender entered its epoch at the mark it began at, which was
        // committed then: its log through it is committed, whatever
        // commit-number the sender knows now, made again from its journal.
        let known = m.commit.max(m.began).min(self.op());
        if known > self.commit {
            self.commit = known;
        }
        self.check_epoch(out);
        if self.epoch < epoch && m.began <= self.commit {
            self.enter_epoch(m.began, Mark { epoch, configuration: m.configuration }, out);
        }
        if self.status == Status::Transfer {
            // Within the epoch after all: the log fetched as a later
            // view's would be.
            self.enter_view(self.commit, out);
            out.push(Output::Event(ReplicaEvent::ViewChanged { view: self.view, primary: self.primary_of(self.view) }));
        }
        if self.status == Status::Normal {
            if self.is_primary() {
                self.deliver(out);
            } else {
                self.acknowledge(out);
            }
            self.after_commit(out);
        }
    }
}
