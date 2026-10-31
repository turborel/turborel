//! \[EXTENSION\] VR's view change [VRR §4.2]: "View changes are used to
//! mask failures of the primary." Its correctness condition: "every
//! operation that has been executed by means of an up-call to the service
//! code at one of the replicas must survive into the new view in the same
//! order selected for it at the time it was executed."
//!
//! \[DESIGN\] Departure: the DOVIEWCHANGE and STARTVIEW messages carry the
//! whole log, as [VRR §4.2] has them, not a suffix as [VRR §5.3] suggests;
//! a suffix, and the primary's asking for more, wait for log retention (a
//! later stage).

use super::frame::{DoViewChange, Message, StartView, StartViewChange};
use super::{Candidate, Change, Lead, Status, Vr};
use crate::{Output, ReplicaEvent};

impl Vr {
    /// [VRR §4.2 step 1]: "A replica i that notices the need for a view
    /// change advances its view-number, sets its status to view-change, and
    /// sends a ⟨STARTVIEWCHANGE v, i⟩ message to the all other replicas,
    /// where v identifies the new view." \[DESIGN\] It writes its
    /// view-number first (D57), so it never takes a PREPARE of an earlier
    /// view again, even after a crash.
    pub(super) fn start_view_change(&mut self, view: u64, out: &mut Vec<Output>) {
        self.depose(out);
        self.view = view;
        self.status = Status::ViewChange;
        self.lead = Lead::default();
        self.change = Change { open: true, ..Change::default() };
        self.timers.heard = 0;
        self.asked = None;
        self.persist_state(out);
        self.send_view_change(out);
    }

    /// A STARTVIEWCHANGE to every other replica, and once enough have come,
    /// a DOVIEWCHANGE.
    pub(super) fn send_view_change(&mut self, out: &mut Vec<Output>) {
        let start = Message::StartViewChange(StartViewChange { view: self.view, replica: self.me });
        self.broadcast(&start, out);
        self.check_started(out);
    }

    /// "A replica notices the need for a view change either based on its
    /// own timer, or because it receives a STARTVIEWCHANGE or DOVIEWCHANGE
    /// message for a view with a larger number than its own view-number"
    /// [VRR §4.2 step 1].
    fn notice(&mut self, view: u64, out: &mut Vec<Output>) -> bool {
        if view > self.view {
            self.start_view_change(view, out);
        }
        view == self.view && self.status == Status::ViewChange && self.change.open
    }

    /// [VRR §4.2 step 2]: "When replica i receives STARTVIEWCHANGE messages
    /// for its view-number from f other replicas, it sends a ⟨DOVIEWCHANGE
    /// v, l, v′, n, k, i⟩ message to the node that will be the primary in
    /// the new view."
    pub(super) fn on_start_view_change(&mut self, m: StartViewChange, out: &mut Vec<Output>) {
        if self.notice(m.view, out) {
            self.change.started.insert(m.replica);
            self.check_started(out);
        }
    }

    /// Sends the DOVIEWCHANGE once f others have started the view change,
    /// and again at each heartbeat until the view starts. "Here v is its
    /// view-number, l is its log, v′ is the view number of the latest view
    /// in which its status was normal, n is the op-number, and k is the
    /// commit-number."
    fn check_started(&mut self, out: &mut Vec<Output>) {
        if self.change.started.len().saturating_add(1) < self.quorum() {
            return;
        }
        self.change.done = true;
        let mine = Candidate { log: self.log.clone(), normal_view: self.normal_view, op: self.op(), commit: self.commit };
        let primary = self.primary_of(self.view);
        if primary == self.me {
            return self.take_candidate(self.me, mine, out);
        }
        let Candidate { log, normal_view, op, commit } = mine;
        let done = DoViewChange { view: self.view, log, normal_view, op, commit, replica: self.me };
        Vr::send(primary, &Message::DoViewChange(done), out);
    }

    /// A DOVIEWCHANGE, to the primary of its view.
    pub(super) fn on_do_view_change(&mut self, m: DoViewChange, out: &mut Vec<Output>) {
        if self.notice(m.view, out) && self.is_primary() {
            let candidate = Candidate { log: m.log, normal_view: m.normal_view, op: m.op, commit: m.commit };
            self.take_candidate(m.replica, candidate, out);
        }
    }

    /// [VRR §4.2 step 3]: "When the new primary receives f + 1
    /// DOVIEWCHANGE messages from different replicas (including itself), it
    /// sets its view-number to that in the messages and selects as the new
    /// log the one contained in the message with the largest v′; if several
    /// messages have the same v′ it selects the one among them with the
    /// largest n. It sets its op-number to that of the topmost entry in the
    /// new log, sets its commit-number to the largest such number it
    /// received in the DOVIEWCHANGE messages, changes its status to normal,
    /// and informs the other replicas of the completion of the view change
    /// by sending ⟨STARTVIEW v, l, n, k⟩ messages to the other replicas".
    fn take_candidate(&mut self, from: crate::ReplicaId, candidate: Candidate, out: &mut Vec<Output>) {
        self.change.candidates.insert(from, candidate);
        if self.change.candidates.len() < self.quorum() || !self.change.candidates.contains_key(&self.me) {
            return;
        }
        let commit = self.change.candidates.values().map(|c| c.commit).max().unwrap_or(0);
        let chosen = self.change.candidates.values().max_by_key(|c| (c.normal_view, c.op)).map(|c| c.log.clone()).unwrap_or_default();
        self.install(chosen, out);
        self.enter_view(commit, out);
        self.lead.inherited = self.op();
        out.push(Output::Event(ReplicaEvent::ViewChanged { view: self.view, primary: self.me }));
        let start = Message::StartView(StartView { view: self.view, log: self.log.clone(), op: self.op(), commit: self.commit });
        self.broadcast(&start, out);
        self.after_commit(out);
        self.try_commit(out);
    }

    /// [VRR §4.2 step 5]: "When other replicas receive the STARTVIEW
    /// message, they replace their log with the one in the message, set
    /// their op-number to that of the latest entry in the log, set their
    /// view-number to the view number in the message, change their status
    /// to normal … If there are non-committed operations in the log, they
    /// send a ⟨PREPAREOK v, n, i⟩ message to the primary; here n is the
    /// op-number. Then they execute all operations known to be committed
    /// that they haven't executed previously, advance their commit-number".
    pub(super) fn on_start_view(&mut self, m: StartView, out: &mut Vec<Output>) {
        let current = m.view == self.view && self.status == Status::Normal;
        if m.view < self.view || current {
            return;
        }
        self.depose(out);
        self.view = m.view;
        self.install(m.log, out);
        self.enter_view(m.commit, out);
        out.push(Output::Event(ReplicaEvent::ViewChanged { view: self.view, primary: self.primary_of(self.view) }));
        if self.op() > self.commit {
            self.acknowledge(out);
        }
        self.after_commit(out);
    }

    /// Replaces the log by `log`: the journal's records after the two agree
    /// removed, and the rest of `log` written. A `PersistState` follows, so
    /// the journal changes at once.
    pub(super) fn install(&mut self, log: Vec<Vec<u8>>, out: &mut Vec<Output>) {
        let agree = self.log.iter().zip(&log).take_while(|(mine, theirs)| mine == theirs).count();
        let kept = u64::try_from(agree).unwrap_or(u64::MAX);
        if self.log.len() > agree {
            out.push(Output::Truncate { after: crate::Lsn(kept) });
        }
        self.durable = self.durable.min(kept);
        self.log.truncate(agree);
        let mut at = kept;
        for record in log.into_iter().skip(agree) {
            at = at.saturating_add(1);
            self.log.push(record.clone());
            self.persist(at, record, out);
        }
    }

    /// The replica becomes normal in its view, its view-number and v′ and
    /// log written at once, knowing `commit` committed.
    pub(super) fn enter_view(&mut self, commit: u64, out: &mut Vec<Output>) {
        self.normal_view = self.view;
        self.status = Status::Normal;
        self.persist_state(out);
        self.commit = self.commit.max(commit).min(self.op());
        self.lead = Lead::default();
        self.change = Change::default();
        self.timers.heard = 0;
        self.asked = None;
    }
}
