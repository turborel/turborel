//! \[EXTENSION\] VR's state transfer of the log [VRR §5.2]: "State transfer
//! is used by a node that has gotten behind (but hasn't crashed) to bring
//! itself up-to-date. There are two cases, depending on whether the slow
//! node has learned that it is missing requests in its current view, or has
//! heard about a later view." A log is a window (RULES.md D57, log
//! retention): a replica behind what another retains is answered NEEDSTATE,
//! and has its driver fetch a state transfer of its database
//! (`Output::FetchState`), told as `Input::Installed` where it then holds
//! the log, from where the log is fetched as before.

use super::frame::{GetState, Message, NeedState, NewState};
use super::{Change, Lead, Status, Vr};
use crate::{Lsn, Output, ReplicaEvent};

impl Vr {
    /// [VRR §5.2]: "To get the state the replica sends a ⟨GETSTATE v, n′,
    /// i⟩ message to one of the other replicas, where v is its view-number
    /// and n′ is its op-number": here to the primary of v, which has every
    /// record of the view. Asked once until answered or a heartbeat passes.
    pub(super) fn get_state(&mut self, op: u64, out: &mut Vec<Output>) {
        if self.asked == Some(op) {
            return;
        }
        self.asked = Some(op);
        let get = Message::GetState(GetState { view: self.view, op, replica: self.me });
        Vr::send(self.primary_of(self.view), &get, out);
    }

    /// [VRR §5.2], of a replica that "has heard about a later view": "In
    /// the latter it needs to learn about requests after the latest
    /// committed request in its log, since requests after that might have
    /// been reordered in the view change, so in this case it sets its
    /// op-number to its commit-number and removes all entries after this
    /// from its log." \[DESIGN\] Departure: it keeps those entries until the
    /// NEWSTATE replaces them, at once with its view (`PersistState`), so a
    /// crash meanwhile leaves its journal as it was, never shortened with
    /// its view unchanged; it takes part in no view meanwhile.
    pub(super) fn later_view(&mut self, view: u64, out: &mut Vec<Output>) {
        self.depose(out);
        self.view = view;
        self.status = Status::Transfer;
        self.lead = Lead::default();
        self.change = Change::default();
        self.timers.heard = 0;
        self.asked = None;
        self.get_state(self.commit, out);
    }

    /// [VRR §5.2]: "A replica responds to a GETSTATE message only if its
    /// status is normal and it is currently in view v. In this case it
    /// sends a ⟨NEWSTATE v, l, n, k⟩ message, where v is its view-number, l
    /// is its log after n′, n is its op-number, and k is its
    /// commit-number."
    pub(super) fn on_get_state(&mut self, m: GetState, out: &mut Vec<Output>) {
        if m.view != self.view || self.status != Status::Normal || m.op > self.op() {
            return;
        }
        // \[DESIGN\] Behind what is retained here: the asker's database must
        // take this one's state first (RULES.md D57).
        if m.op < self.log.first() {
            return Vr::send(m.replica, &Message::NeedState(NeedState { view: self.view, op: m.op }), out);
        }
        let log = self.log.after(m.op);
        let new = Message::NewState(NewState { view: self.view, after: m.op, log, op: self.op(), commit: self.commit });
        Vr::send(m.replica, &new, out);
    }

    /// \[EXTENSION\] A NEEDSTATE from the primary of the view, answering
    /// the GETSTATE still waited on: the driver is to fetch a state
    /// transfer from it. One answering an older GETSTATE, or one a
    /// heartbeat has passed since, tells nothing: this replica may have
    /// moved on, and asks again if it must; so the transfer fetched is
    /// always past this log, which cannot begin again before where it
    /// does.
    pub(super) fn on_need_state(&mut self, m: NeedState, out: &mut Vec<Output>) {
        if m.view != self.view || self.status == Status::ViewChange || self.asked != Some(m.op) {
            return;
        }
        self.fetch_state(out);
    }

    /// \[EXTENSION\] Has the driver fetch a state transfer from the primary
    /// of the view (RULES.md D57): this replica takes part in nothing until
    /// it is installed, its log then fetched after it.
    pub(super) fn fetch_state(&mut self, out: &mut Vec<Output>) {
        self.depose(out);
        self.status = Status::Transfer;
        self.lead = Lead::default();
        self.change = Change::default();
        self.timers.heard = 0;
        self.asked = None;
        out.push(Output::FetchState { from: self.primary_of(self.view) });
    }

    /// \[EXTENSION\] `Input::Installed`: the database took a state transfer
    /// taken at `at`, so it holds the log through it in place of what it
    /// held: the journal begins after it, a record in doubt through it has
    /// an outcome no log can tell, and what is committed after it is given
    /// the database again. Fetching still, the log after `at` is fetched
    /// next; moved on meanwhile, to a view change or a view, the log after
    /// `at` is kept, as it may be acknowledged.
    pub(super) fn installed(&mut self, at: u64, out: &mut Vec<Output>) {
        let fetching = self.status == Status::Transfer;
        if fetching && self.op() > at {
            out.push(Output::Truncate { after: Lsn(at) });
            self.log.truncate_to(at);
        }
        self.log.retain_after(at);
        out.push(Output::Retain { through: Lsn(at) });
        // A record in doubt at a position let go: whether the log kept it
        // no log can tell now.
        while let Some(entry) = self.in_doubt.first_entry() {
            if *entry.key() > at {
                break;
            }
            out.push(Output::Outcome { at: Lsn(entry.remove_entry().0), kept: None });
        }
        self.commit = self.commit.max(at);
        self.durable = self.durable.max(at).min(self.op());
        self.delivered = at;
        self.applied = at;
        self.persist_state(out);
        if fetching {
            self.asked = None;
            self.get_state(at, out);
        } else {
            self.deliver(out);
        }
    }

    /// [VRR §5.2]: "When replica i receives the NEWSTATE message, it
    /// appends the log in the message to its log and updates its state
    /// using the other information in the message." A backup of the view
    /// skips the records it holds: in one view they are the same.
    pub(super) fn on_new_state(&mut self, m: NewState, out: &mut Vec<Output>) {
        if m.view != self.view {
            return;
        }
        match self.status {
            Status::Normal => self.append_state(m, out),
            Status::Transfer => self.take_state(m, out),
            Status::ViewChange | Status::Recovering => {}
        }
    }

    /// A NEWSTATE to a backup of the view: the records past its log
    /// appended.
    fn append_state(&mut self, m: NewState, out: &mut Vec<Output>) {
        if self.is_primary() || m.after > self.op() {
            return;
        }
        self.asked = None;
        let mut at = m.after;
        for record in m.log {
            at = at.saturating_add(1);
            if at > self.op() {
                self.log.push(record.clone());
                self.persist(at, record, out);
            }
        }
        self.learn_commit(m.commit, out);
    }

    /// A NEWSTATE to a replica fetching a later view's log: its log after
    /// `after` replaced, `after` being within what it knows committed, so
    /// it holds the log of the replica that answered; then it is normal in
    /// the view, as STARTVIEW would have made it.
    fn take_state(&mut self, m: NewState, out: &mut Vec<Output>) {
        if m.after > self.commit {
            return;
        }
        if !self.install(m.after, m.log, out) {
            return self.fetch_state(out);
        }
        self.enter_view(m.commit, out);
        out.push(Output::Event(ReplicaEvent::ViewChanged { view: self.view, primary: self.primary_of(self.view) }));
        self.acknowledge(out);
        self.after_commit(out);
    }
}
