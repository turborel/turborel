//! \[EXTENSION\] VR's normal operation [VRR §4.1]: the primary orders
//! records, backups prepare them in order, and the primary commits a record
//! once a quorum holds it durably. "Replicas participate in processing of
//! client requests only when their status is normal."

use std::cmp::Ordering;

use super::frame::{Commit, Message, Prepare, PrepareOk};
use super::{Status, Vr};
use crate::{Lsn, Output, ReplicaEvent, Role};

impl Vr {
    /// [VRR §4.1 step 3]: "The primary advances op-number, adds the request
    /// to the end of the log … Then it sends a ⟨PREPARE v, m, n, k⟩ message
    /// to the other replicas". The record is also written to the primary's
    /// journal; it counts toward a quorum once durable.
    pub(super) fn append(&mut self, record: Vec<u8>, out: &mut Vec<Output>) {
        if !self.lead.serving {
            out.push(Output::Event(ReplicaEvent::AppendRefused));
            return;
        }
        self.log.push(record.clone());
        let op = self.op();
        out.push(Output::Assigned(Lsn(op)));
        self.persist(op, record.clone(), out);
        self.own.insert(op, record.clone());
        let prepare = Message::Prepare(Prepare { view: self.view, record, op, commit: self.commit });
        self.broadcast(&prepare, out);
    }

    /// [VRR §4.1 step 4]: "Backups process PREPARE messages in order: a
    /// backup won't accept a prepare with op-number n until it has entries
    /// for all earlier requests in its log. When a backup i receives a
    /// PREPARE message, it waits until it has entries in its log for all
    /// earlier requests (doing state transfer if necessary to get the
    /// missing information). Then it increments its op-number, adds the
    /// request to the end of its log … and sends a ⟨PREPAREOK v, n, i⟩
    /// message to the primary". \[DESIGN\] Departure: the PREPAREOK waits
    /// for the record to be durable (`persisted`). A PREPARE already held
    /// is answered again, as its PREPAREOK may have been lost.
    pub(super) fn on_prepare(&mut self, m: Prepare, out: &mut Vec<Output>) {
        if !self.in_view(m.view, out) {
            return;
        }
        let next = self.op().saturating_add(1);
        match m.op.cmp(&next) {
            Ordering::Greater => self.get_state(self.op(), out),
            Ordering::Equal => {
                self.log.push(m.record.clone());
                self.persist(m.op, m.record, out);
            }
            Ordering::Less => self.acknowledge(out),
        }
        self.learn_commit(m.commit, out);
    }

    /// Whether a backup takes a PREPARE or COMMIT of `view` from its
    /// primary. [VRR §4.1]: "Replicas only process normal protocol messages
    /// containing a view-number that matches the view-number they know. If
    /// the sender is behind, the receiver drops the message. If the sender
    /// is ahead, the replica performs a state transfer". So too a replica in
    /// a view change for `view` whose primary is already normal in it: its
    /// STARTVIEW was lost.
    fn in_view(&mut self, view: u64, out: &mut Vec<Output>) -> bool {
        if view < self.view {
            return false;
        }
        let behind = view > self.view || self.status == Status::ViewChange;
        if behind {
            self.later_view(view, out);
            return false;
        }
        if self.status == Status::Normal {
            self.timers.heard = 0;
        }
        self.status == Status::Normal
    }

    /// ⟨PREPAREOK v, n, i⟩ for every record durable, which "indicate[s]
    /// that this operation and all earlier ones have prepared locally"
    /// [VRR §4.1 step 4].
    pub(super) fn acknowledge(&self, out: &mut Vec<Output>) {
        let op = self.durable.min(self.op());
        let ok = Message::PrepareOk(PrepareOk { view: self.view, op, replica: self.me });
        Vr::send(self.primary_of(self.view), &ok, out);
    }

    /// [VRR §4.1 step 5]: "The primary waits for f PREPAREOK messages from
    /// different backups; at this point it considers the operation (and all
    /// earlier ones) to be committed."
    pub(super) fn on_prepare_ok(&mut self, m: PrepareOk, out: &mut Vec<Output>) {
        if m.view != self.view || self.status != Status::Normal || !self.is_primary() {
            return;
        }
        let op = m.op.min(self.op());
        let acked = self.lead.acked.entry(m.replica).or_default();
        *acked = op.max(*acked);
        self.try_commit(out);
    }

    /// The highest op-number a quorum holds durably, counting this replica
    /// once its own copy is durable, committed.
    pub(super) fn try_commit(&mut self, out: &mut Vec<Output>) {
        let mut commit = self.commit;
        while commit < self.op() {
            let next = commit.saturating_add(1);
            let backups = self.lead.acked.values().filter(|&&acked| acked >= next).count();
            let holders = backups.saturating_add(usize::from(self.durable >= next));
            if holders < self.quorum() {
                break;
            }
            commit = next;
        }
        if commit > self.commit {
            self.commit = commit;
            self.after_commit(out);
        }
    }

    /// [VRR §4.1 step 7]: "When a backup learns of a commit, it waits until
    /// it has the request in its log (which may require state transfer) and
    /// until it has executed all earlier operations." A backup's log agrees
    /// with its primary's through its op-number in a view, so what is
    /// committed of it is known at once.
    pub(super) fn learn_commit(&mut self, commit: u64, out: &mut Vec<Output>) {
        if commit > self.op() {
            self.get_state(self.op(), out);
        }
        let known = commit.min(self.op());
        if known > self.commit {
            self.commit = known;
            self.after_commit(out);
        }
    }

    /// [VRR §4.1 step 6]: the primary's ⟨COMMIT v, k⟩.
    pub(super) fn on_commit(&mut self, m: Commit, out: &mut Vec<Output>) {
        if self.in_view(m.view, out) {
            self.learn_commit(m.commit, out);
        }
    }

    /// What follows a commit-number advanced: a serving primary's database
    /// told, records in doubt resolved, and the records delivered.
    pub(super) fn after_commit(&mut self, out: &mut Vec<Output>) {
        if self.lead.serving {
            out.push(Output::Committed(Lsn(self.commit)));
            let commit = self.commit;
            self.own.retain(|&at, _| at > commit);
        }
        while let Some(entry) = self.in_doubt.first_entry() {
            if *entry.key() > self.commit {
                break;
            }
            let (at, record) = entry.remove_entry();
            let kept = self.record(at) == Some(&record);
            out.push(Output::Outcome { at: Lsn(at), kept });
        }
        self.deliver(out);
    }

    /// The records committed and durable here delivered to the database,
    /// but to a serving primary's, which wrote them, and to a new primary's
    /// only through the log it took over.
    pub(super) fn deliver(&mut self, out: &mut Vec<Output>) {
        if self.lead.serving {
            return;
        }
        let mut through = self.commit.min(self.durable);
        if self.status == Status::Normal && self.is_primary() {
            through = through.min(self.lead.inherited);
        }
        if through > self.delivered {
            self.delivered = through;
            out.push(Output::Deliver { through: Lsn(through) });
        }
    }

    /// The journal made the record at `at` durable: one `Persisted` for
    /// each `Persist`, in order. One answering a write a `PersistState` has
    /// made durable since tells nothing. A backup acknowledges; a primary
    /// counts its own copy.
    pub(super) fn persisted(&mut self, at: u64, out: &mut Vec<Output>) {
        self.writes.taken = self.writes.taken.saturating_add(1);
        let stale = self.writes.taken <= self.writes.synced;
        if stale || at != self.durable.saturating_add(1) || at > self.op() {
            return;
        }
        self.durable = at;
        if self.status != Status::Normal {
            return self.deliver(out);
        }
        if self.is_primary() {
            self.try_commit(out);
        } else {
            self.acknowledge(out);
            self.deliver(out);
        }
    }

    /// The timer. A primary sends its heartbeat. A backup counts how long
    /// it has not heard from its primary, and a replica how long its view
    /// change or state transfer has lasted: past the timeout, it starts a
    /// view change to the next view [VRR §4.2]: "A view change may not
    /// succeed, e.g., because the new primary fails. In this case the
    /// replicas will start a further view change". Meanwhile, at each
    /// heartbeat, it sends again what it is waiting on an answer to.
    pub(super) fn tick(&mut self, out: &mut Vec<Output>) {
        self.timers.beat = self.timers.beat.saturating_add(1);
        let beat = self.timers.beat >= self.options.heartbeat_ticks;
        if beat {
            self.timers.beat = 0;
            self.asked = None;
        }
        let leading = self.status == Status::Normal && self.is_primary();
        if leading {
            if beat {
                self.heartbeat(out);
            }
            return;
        }
        self.timers.heard = self.timers.heard.saturating_add(1);
        if self.timers.heard >= self.options.view_timeout_ticks {
            return self.start_view_change(self.view.saturating_add(1), out);
        }
        if beat {
            self.send_again(out);
        }
    }

    /// What a replica waiting on others sends again at a heartbeat.
    fn send_again(&mut self, out: &mut Vec<Output>) {
        match self.status {
            Status::Normal => {}
            Status::ViewChange => {
                if self.change.open {
                    self.send_view_change(out);
                }
            }
            Status::Transfer => self.get_state(self.commit, out),
        }
    }

    /// To each backup, the records past the last it acknowledged, or if it
    /// has them all, ⟨COMMIT v, k⟩ [VRR §4.1 step 6].
    fn heartbeat(&self, out: &mut Vec<Output>) {
        for backup in self.others() {
            let acked = self.lead.acked.get(&backup).copied().unwrap_or(0);
            if acked >= self.op() {
                Vr::send(backup, &Message::Commit(Commit { view: self.view, commit: self.commit }), out);
                continue;
            }
            let last = acked.saturating_add(self.options.resend_records).min(self.op());
            for op in acked.saturating_add(1)..=last {
                if let Some(record) = self.record(op) {
                    let prepare = Prepare { view: self.view, record: record.clone(), op, commit: self.commit };
                    Vr::send(backup, &Message::Prepare(prepare), out);
                }
            }
        }
    }

    /// The primary of a view begins to serve, its database to append, once
    /// the log it took over is committed and delivered.
    pub(super) fn serve(&mut self, out: &mut Vec<Output>) {
        let inherited = self.lead.inherited;
        let ready = self.commit >= inherited && self.delivered >= inherited;
        if self.status == Status::Normal && self.is_primary() && !self.lead.serving && ready {
            self.lead.serving = true;
            out.push(Output::Role(Role::Primary { view: self.view }));
        }
    }

    /// A serving primary leaves its service: its records not committed are
    /// in doubt, and its database, deposed, opens again from its last
    /// checkpoint, so what is committed is delivered again from the start.
    pub(super) fn depose(&mut self, out: &mut Vec<Output>) {
        if self.lead.serving {
            self.lead.serving = false;
            out.push(Output::Role(Role::Backup));
            self.in_doubt.append(&mut self.own);
            self.delivered = 0;
        }
    }
}
