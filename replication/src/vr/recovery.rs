//! \[EXTENSION\] VR's recovery of a replica whose journal holds no state
//! [VRR §4.3] (RULES.md D57): lost, or never written, as a member's that
//! has not run. It cannot take part in a view or a view change until it
//! has a state at least as recent as it had: it sends RECOVERY with a
//! nonce to every other member, and waits for RECOVERYRESPONSE from f + 1
//! of them, each echoing the nonce, among them one from the primary of
//! the latest view they name, whose log it takes; then it is normal in
//! that view. A member answers only while normal, and as primary with its
//! log.
//!
//! \[DESIGN\] Departures:
//! * The log taken is a window (log retention): a database holding the
//!   log past the window's first position takes the window and follows
//!   from where it holds, the journal written to hold the window; one
//!   behind the window fetches a state transfer of the primary's database
//!   first (`transfer.rs`), recovering still, so it takes part in no view
//!   change with a log it has not got, and recovers again, from the
//!   primary's log, once the state is installed.
//! * A new group's replicas are made normal in view 0 ([`Vr::new`]), not
//!   recovering: a replica with no journal and a database holding nothing
//!   recovers like any other, waiting for a primary to answer, as it may
//!   be a new member joined by a reconfiguration, or a member whose
//!   journal and database were both lost, of a group that holds a log
//!   elsewhere [VRR §7], which members as empty as it could not tell
//!   from a new group. A group of one recovers at once, from its database
//!   alone. One in a view change answers too, with the view it was last
//!   normal in and, as its primary, its log, so the view change, which
//!   waits on the members recovering, goes on.
//! * A replica whose journal holds records but no state is treated as
//!   having lost it: the state is written first, as the replica is opened
//!   (`normal.rs`, `opened`).

use std::collections::BTreeMap;

use super::frame::{Message, PrimaryLog, Recovery, RecoveryResponse};
use super::window::Window;
use super::{Change, Lead, Status, Vr};
use crate::{Lsn, Output, ReplicaEvent, ReplicaId};

/// What a replica recovering keeps.
#[derive(Debug, Default)]
pub(super) struct Recovering {
    /// The nonce its RECOVERY carries, which the responses echo.
    pub(super) nonce: u64,
    /// The responses, by replica.
    responses: BTreeMap<ReplicaId, RecoveryResponse>,
}

impl Vr {
    /// `Input::Opened` of a replica recovering, its database holding the
    /// log through `at`: its RECOVERY sent; a group of one recovers at
    /// once.
    pub(super) fn begin_recovery(&mut self, out: &mut Vec<Output>) {
        out.push(Output::Event(ReplicaEvent::Recovering));
        if self.members.len() == 1 {
            return self.recover_alone(out);
        }
        self.send_recovery(out);
    }

    /// Recovers again, the state fetched: the responses before forgotten,
    /// a RECOVERY of a new nonce sent, whose responses the recovery takes.
    pub(super) fn recover_again(&mut self, out: &mut Vec<Output>) {
        self.recovering.responses.clear();
        self.recovering.nonce = self.recovering.nonce.wrapping_add(1);
        self.send_recovery(out);
    }

    /// ⟨RECOVERY i, x⟩ to every other member.
    pub(super) fn send_recovery(&self, out: &mut Vec<Output>) {
        let recovery = Message::Recovery(Recovery { replica: self.me, nonce: self.recovering.nonce });
        self.broadcast(&recovery, out);
    }

    /// A RECOVERY from `m.replica`: answered while normal, as primary with
    /// the log; while in a view change, with the view this replica was
    /// last normal in, and its log if it was that view's primary, which
    /// it holds whole until the new view is installed, so that members
    /// recovering take part in the view change, which waits on them;
    /// while leaving, with the epoch and its members, which a member
    /// recovering from behind the epoch adopts; in no other status.
    pub(super) fn on_recovery(&mut self, m: Recovery, out: &mut Vec<Output>) {
        if !matches!(self.status, Status::Normal | Status::ViewChange | Status::Leaving) {
            return;
        }
        let view = self.normal_view;
        let log = (self.primary_of(view) == self.me).then(|| self.primary_log());
        let response = RecoveryResponse {
            view,
            nonce: m.nonce,
            replica: self.me,
            configuration: self.configuration.clone(),
            began: self.epoch_began,
            log,
        };
        self.send(m.replica, &Message::RecoveryResponse(response), out);
    }

    /// This primary's log, as a window, for a replica recovering.
    fn primary_log(&self) -> PrimaryLog {
        let after = self.log.first();
        PrimaryLog { after, log: self.log.after(after), marks: self.log.marks_after(after), op: self.op(), commit: self.commit }
    }

    /// A RECOVERYRESPONSE, sent in `epoch`: kept if it echoes the nonce of
    /// this recovery; one of a later epoch than this replica knows has it
    /// adopt the epoch and its configuration, the responses of earlier
    /// epochs let go.
    pub(super) fn on_recovery_response(&mut self, m: RecoveryResponse, epoch: u64, out: &mut Vec<Output>) {
        if self.status != Status::Recovering || m.nonce != self.recovering.nonce || epoch < self.epoch {
            return;
        }
        if epoch > self.epoch {
            self.epoch = epoch;
            self.epoch_began = m.began;
            self.members = super::members_of(&m.configuration);
            self.configuration.clone_from(&m.configuration);
            self.recovering.responses.clear();
            out.push(Output::Epoch { epoch, configuration: m.configuration.clone() });
            out.push(Output::Event(ReplicaEvent::Reconfigured { epoch, configuration: m.configuration.clone() }));
            if !self.members.contains(&self.me) {
                // No member of the epoch: leaving it, with nothing to
                // recover, until the members are normal in it.
                self.status = Status::Leaving;
                self.persist_state(out);
                return;
            }
        }
        // Only a member's answer counts: an answerer outside the group,
        // which this replica's setting may name, is no part of its
        // quorums, and told only the epoch.
        if !self.members.contains(&m.replica) {
            return;
        }
        self.recovering.responses.insert(m.replica, m);
        self.check_recovered(out);
    }

    /// Whether the responses complete the recovery: f + 1 of them, the
    /// primary of the latest view's among them, of that view, with its
    /// log: an earlier answer of the same replica, of an earlier view it
    /// was primary of, is no log of the latest view; the primary answers
    /// again at this replica's next RECOVERY.
    fn check_recovered(&mut self, out: &mut Vec<Output>) {
        let f = self.members.len().saturating_sub(self.quorum());
        let responses = &self.recovering.responses;
        let latest = responses.values().map(|r| r.view).max().unwrap_or(0);
        let primary = self.primary_of(latest);
        if responses.len() > f
            && let Some(log) = responses.get(&primary).filter(|r| r.view == latest).and_then(|r| r.log.clone())
        {
            self.recover_from(latest, log, out);
        }
    }

    /// Recovers from the primary of `view`'s log: see the module's
    /// documentation. A log ending before what the database holds is no
    /// state to recover from: the recovery waits, for the operator.
    fn recover_from(&mut self, view: u64, taken: PrimaryLog, out: &mut Vec<Output>) {
        let held = self.applied;
        if held > taken.op {
            return;
        }
        self.view = view;
        self.lead = Lead::default();
        self.change = Change::default();
        self.timers.heard = 0;
        self.asked = None;
        if held < taken.after {
            // Recovering still, taking part in nothing: recovered again,
            // from the primary's log, once the state is installed.
            self.fetch_state(out);
            self.status = Status::Recovering;
            return;
        }
        // The log is taken from where this replica's window begins at
        // the earliest: the records before it were let go, which its
        // journal cannot take back, and its database holds them.
        let first = self.log.first();
        let begin = taken.after.max(first);
        if self.op() > begin {
            out.push(Output::Truncate { after: Lsn(begin) });
        }
        if begin > first {
            out.push(Output::Retain { through: Lsn(begin) });
        }
        self.log = Window::new(begin, Vec::new(), BTreeMap::new());
        let skip = usize::try_from(begin.saturating_sub(taken.after)).unwrap_or(usize::MAX);
        for (index, record) in taken.log.into_iter().enumerate().skip(skip) {
            let at = taken.after.saturating_add(u64::try_from(index).unwrap_or(u64::MAX)).saturating_add(1);
            self.log.push(record.clone());
            if let Some(mark) = taken.marks.get(&at) {
                self.log.set_mark(at, mark.clone());
                out.push(Output::Mark { at: Lsn(at), mark: mark.clone() });
            }
            self.persist(at, record, out);
        }
        self.epoch_began = self.epoch_began.max(taken.after.min(self.commit));
        self.commit = taken.commit.max(held).min(self.op());
        self.recovered(out);
        out.push(Output::Event(ReplicaEvent::Recovered { view, from: self.primary_of(view) }));
        self.acknowledge(out);
        self.after_commit(out);
    }

    /// A replica alone, the group's one member: normal in view 0, its log
    /// beginning where its database holds it, as no other holds more; it
    /// serves once the records it holds are committed and delivered.
    fn recover_alone(&mut self, out: &mut Vec<Output>) {
        let held = self.applied;
        if held > self.log.first() {
            self.log.retain_after(held);
            out.push(Output::Retain { through: Lsn(held) });
        }
        self.view = 0;
        self.commit = held.max(self.commit);
        self.lead = Lead { inherited: self.op(), ..Lead::default() };
        self.change = Change::default();
        self.recovered(out);
        out.push(Output::Event(ReplicaEvent::Recovered { view: 0, from: self.me }));
        if self.is_primary() {
            self.try_commit(out);
        }
        self.after_commit(out);
    }

    /// Normal in its view, its state written, the view told.
    fn recovered(&mut self, out: &mut Vec<Output>) {
        self.normal_view = self.view;
        self.status = Status::Normal;
        self.source = None;
        self.recovering = Recovering::default();
        self.persist_state(out);
        out.push(Output::Event(ReplicaEvent::ViewChanged { view: self.view, primary: self.primary_of(self.view) }));
    }
}
