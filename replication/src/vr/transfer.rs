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
use super::window::Window;
use super::{Change, Lead, Status, Vr};
use crate::{Lsn, Output, ReplicaEvent, ReplicaId};
use std::collections::BTreeMap;

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
        let to = self.source.unwrap_or_else(|| self.primary_of(self.view));
        if to == self.me {
            // The primary of its view, deposed by a message of a later
            // epoch from a replica it cannot reach: every other member is
            // asked, those in the later epoch, or leaving it, answering.
            for other in self.others() {
                self.send(other, &get, out);
            }
        } else {
            self.send(to, &get, out);
        }
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
        self.source = None;
        self.get_state(self.commit, out);
    }

    /// [VRR §5.2]: "A replica responds to a GETSTATE message only if its
    /// status is normal and it is currently in view v. In this case it
    /// sends a ⟨NEWSTATE v, l, n, k⟩ message, where v is its view-number, l
    /// is its log after n′, n is its op-number, and k is its
    /// commit-number." \[EXTENSION\] One leaving answers too, and an asker
    /// of an earlier epoch is answered whatever its view, by one in a view
    /// change too, the NEWSTATE carrying the epoch's configuration
    /// (`vr/reconfigure.rs`).
    pub(super) fn on_get_state(&mut self, m: GetState, epoch: u64, out: &mut Vec<Output>) {
        // An asker behind the epoch, or behind the mark the epoch began
        // at, needs the log through the mark, committed, the same in every
        // view: one in a view change answers it too, its log whole from its
        // last normal view, as the view change may wait on the asker.
        let behind = epoch < self.epoch || m.op < self.epoch_began;
        let answering = matches!(self.status, Status::Normal | Status::Leaving) || (self.status == Status::ViewChange && behind);
        // One behind its epoch, its commit-number before the mark the
        // epoch began at, has no log of the epoch to give.
        if (m.view != self.view && !behind) || !answering || m.op > self.op() || self.behind_epoch() {
            return;
        }
        // \[DESIGN\] Behind what is retained here: the asker's database must
        // take this one's state first (RULES.md D57).
        if m.op < self.log.first() {
            let need = NeedState { view: self.view, op: m.op, configuration: self.configuration.clone(), began: self.epoch_began };
            return self.send(m.replica, &Message::NeedState(need), out);
        }
        let (log, marks) = (self.log.after(m.op), self.log.marks_after(m.op));
        let configuration = self.configuration.clone();
        let new = Message::NewState(NewState {
            view: self.view,
            after: m.op,
            log,
            marks,
            op: self.op(),
            commit: self.commit,
            configuration,
            began: self.epoch_began,
        });
        self.send(m.replica, &new, out);
    }

    /// \[EXTENSION\] A NEEDSTATE from the primary of the view, answering
    /// the GETSTATE still waited on: the driver is to fetch a state
    /// transfer from it. One answering an older GETSTATE, or one a
    /// heartbeat has passed since, tells nothing: this replica may have
    /// moved on, and asks again if it must; so the transfer fetched is
    /// always past this log, which cannot begin again before where it
    /// does.
    pub(super) fn on_need_state(&mut self, from: ReplicaId, m: &NeedState, out: &mut Vec<Output>) {
        if m.view != self.view || self.status == Status::ViewChange || self.asked != Some(m.op) {
            return;
        }
        self.fetch_state_from(from, out);
    }

    /// ⟨GETSTATE v, n′, i⟩ to every other member, for `op`, as the primary
    /// of the view may be gone: any normal in the view answers.
    pub(super) fn get_state_from_all(&mut self, op: u64, out: &mut Vec<Output>) {
        self.asked = Some(op);
        let get = Message::GetState(GetState { view: self.view, op, replica: self.me });
        for other in self.others() {
            self.send(other, &get, out);
        }
    }

    /// \[EXTENSION\] Has the driver fetch a state transfer from the primary
    /// of the view (RULES.md D57): this replica takes part in nothing until
    /// it is installed, its log then fetched after it.
    pub(super) fn fetch_state(&mut self, out: &mut Vec<Output>) {
        self.fetch_state_from(self.primary_of(self.view), out);
    }

    /// \[EXTENSION\] Has the driver fetch a state transfer from `from`,
    /// which the log after it is fetched from too.
    pub(super) fn fetch_state_from(&mut self, from: ReplicaId, out: &mut Vec<Output>) {
        self.depose(out);
        self.status = Status::Transfer;
        self.lead = Lead::default();
        self.change = Change::default();
        self.timers.heard = 0;
        self.asked = None;
        self.source = Some(from);
        out.push(Output::FetchState { from });
    }

    /// \[EXTENSION\] `Input::Installed`: the database took a state transfer
    /// whose database holds the log through `at`, so it holds the log
    /// through it in place of what it held: the log begins again after
    /// it, before or after where it began, holding the exporter's
    /// `records` after it, committed all, which the database is given
    /// again, and the journal follows; a record in doubt through `at` has
    /// an outcome no log can tell. Fetching still, the log after the
    /// records is fetched next; recovering, the recovery goes on from
    /// what is held now (`recovery.rs`); moved on meanwhile, to a view
    /// change or a view, the log after them is kept, as it may be
    /// acknowledged.
    pub(super) fn installed(&mut self, at: u64, records: Vec<Vec<u8>>, out: &mut Vec<Output>) {
        let fetching = self.status == Status::Transfer;
        // \[DESIGN\] The log after `at` is kept: a replica made again from
        // its journal knows not which of its records are committed, so
        // none is cut, as a view change may count on it [VRR §4.2]; the
        // exporter's records, committed, replace what differs from them,
        // the rest staying until a NEWSTATE or a view change replaces it.
        if at < self.log.first() {
            self.log = Window::new(at, Vec::new(), BTreeMap::new());
        } else {
            self.log.retain_after(at);
        }
        out.push(Output::Retain { through: Lsn(at) });
        let agree = self.log.agreeing(at, &records);
        let kept = at.saturating_add(u64::try_from(agree).unwrap_or(u64::MAX));
        if records.len() > agree && self.op() > kept {
            out.push(Output::Truncate { after: Lsn(kept) });
            self.log.truncate_to(kept);
            self.durable = self.durable.min(kept);
        }
        let mut through = kept;
        for record in records.into_iter().skip(agree) {
            through = through.saturating_add(1);
            self.log.push(record.clone());
            self.persist(through, record, out);
        }
        let through = at.saturating_add(u64::try_from(agree).unwrap_or(u64::MAX)).max(through);
        // A record in doubt at a position let go: whether the log kept it
        // no log can tell now.
        while let Some(entry) = self.in_doubt.first_entry() {
            if *entry.key() > at {
                break;
            }
            out.push(Output::Outcome { at: Lsn(entry.remove_entry().0), kept: None });
        }
        // A transfer behind what was known committed, the log cut to it:
        // followed from it, the commit-number learned again.
        self.commit = self.commit.max(through).min(self.op());
        self.delivered = at;
        self.applied = at;
        if self.status == Status::Recovering {
            // Recovering, the state fetched first: recovered again, by a
            // RECOVERY of a new nonce, from where the database holds now;
            // no state written, as a journal holding records and no state
            // is one to recover from (`recovery.rs`).
            return self.recover_again(out);
        }
        self.persist_state(out);
        if fetching {
            self.asked = None;
            self.get_state(through, out);
        } else {
            self.deliver(out);
        }
    }

    /// [VRR §5.2]: "When replica i receives the NEWSTATE message, it
    /// appends the log in the message to its log and updates its state
    /// using the other information in the message." A backup of the view
    /// skips the records it holds: in one view they are the same.
    pub(super) fn on_new_state(&mut self, m: NewState, out: &mut Vec<Output>) {
        if m.view < self.view && !(self.status == Status::Transfer && self.behind_epoch()) {
            return;
        }
        match self.status {
            Status::Normal if m.view == self.view => self.append_state(m, out),
            Status::Transfer if m.view >= self.view => {
                // \[DESIGN\] Fetching, a replica takes the log of a later
                // view too: the view moved on as it fetched, and the log
                // after its commit-number is the later view's all the same.
                self.view = m.view;
                self.take_state(m, out);
            }
            Status::Transfer if self.behind_epoch() => {
                // Behind the mark its epoch began at, it takes the log of
                // an earlier view too, through the mark, committed: no
                // longer behind, it takes part in the view change the
                // group is in, which waits on it.
                if m.after <= self.commit && self.install(m.after, (m.log, m.marks), out) {
                    self.asked = None;
                    let known = m.commit.min(self.op());
                    self.commit = self.commit.max(known);
                    self.after_commit(out);
                }
            }
            Status::Normal | Status::Transfer | Status::ViewChange | Status::Recovering | Status::Leaving | Status::Removed => {}
        }
    }

    /// A NEWSTATE to a backup of the view: the records past its log
    /// appended, with the marks among them.
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
                if let Some(mark) = m.marks.get(&at) {
                    self.log.set_mark(at, mark.clone());
                    out.push(Output::Mark { at: Lsn(at), mark: mark.clone() });
                }
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
        if !self.install(m.after, (m.log, m.marks), out) {
            return self.fetch_state(out);
        }
        self.enter_view(m.commit, out);
        out.push(Output::Event(ReplicaEvent::ViewChanged { view: self.view, primary: self.primary_of(self.view) }));
        self.acknowledge(out);
        self.after_commit(out);
    }
}
