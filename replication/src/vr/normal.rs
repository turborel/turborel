//! \[EXTENSION\] VR's normal operation [VRR §4.1]: the primary orders
//! records, backups prepare them in order, and the primary commits a record
//! once a quorum holds it durably. "Replicas participate in processing of
//! client requests only when their status is normal."

use std::cmp::Ordering;

use super::frame::{Commit, Message, Prepare, PrepareOk};
use super::{Status, Vr};
use crate::{Configuration, Lsn, Mark, Output, ReplicaEvent, Role};

impl Vr {
    /// [VRR §4.1 step 3]: "The primary advances op-number, adds the request
    /// to the end of the log … Then it sends a ⟨PREPARE v, m, n, k⟩ message
    /// to the other replicas". The record is also written to the primary's
    /// journal; it counts toward a quorum once durable.
    pub(super) fn append(&mut self, record: Vec<u8>, out: &mut Vec<Output>) {
        if !self.lead.serving || self.reconfiguring() {
            out.push(Output::Event(ReplicaEvent::AppendRefused));
            return;
        }
        self.append_marked(record, None, out);
    }

    /// Appends `record`, marked a reconfiguration to `configuration`,
    /// beginning the next epoch, if one is given (`vr/reconfigure.rs`):
    /// the PREPARE carries the mark.
    pub(super) fn append_marked(&mut self, record: Vec<u8>, configuration: Option<Configuration>, out: &mut Vec<Output>) {
        self.log.push(record.clone());
        let op = self.op();
        out.push(Output::Assigned(Lsn(op)));
        let mark = configuration.map(|configuration| Mark { epoch: self.epoch.saturating_add(1), configuration });
        // \[DESIGN\] The mark is written before its record: a journal keeps
        // a prefix of its writes through a crash, and the record is
        // acknowledged once durable, so a mark written after it could be
        // lost with the record kept, and the record's reconfiguration
        // forgotten; a mark kept without its record is void.
        if let Some(mark) = &mark {
            self.log.set_mark(op, mark.clone());
            out.push(Output::Mark { at: Lsn(op), mark: mark.clone() });
        }
        self.persist(op, record.clone(), out);
        self.own.insert(op, record.clone());
        let prepare = Message::Prepare(Prepare { view: self.view, record, op, commit: self.commit, mark });
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
                if let Some(mark) = m.mark {
                    self.log.set_mark(m.op, mark.clone());
                    out.push(Output::Mark { at: Lsn(m.op), mark });
                }
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
        // Heard from the primary, normal or fetching its state.
        self.timers.heard = 0;
        self.status == Status::Normal
    }

    /// ⟨PREPAREOK v, n, i⟩ for every record durable, which "indicate[s]
    /// that this operation and all earlier ones have prepared locally"
    /// [VRR §4.1 step 4].
    pub(super) fn acknowledge(&self, out: &mut Vec<Output>) {
        if self.is_primary() {
            return;
        }
        let op = self.durable.min(self.op());
        let ok = Message::PrepareOk(PrepareOk { view: self.view, op, replica: self.me });
        self.send(self.primary_of(self.view), &ok, out);
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
            // Holding records durably past the commit-number, the backup
            // acknowledges them again: the primary of a new view, or one
            // whose window the backup's acknowledgement is before, waits
            // on it (`heartbeat`).
            if self.durable > self.commit && self.status == Status::Normal {
                self.acknowledge(out);
            }
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
            out.push(Output::Outcome { at: Lsn(at), kept: Some(kept) });
        }
        self.deliver(out);
        self.check_epoch(out);
    }

    /// The records committed and durable here delivered to the database,
    /// but to a serving primary's, which wrote them, and to a new primary's
    /// only through the log it took over.
    pub(super) fn deliver(&mut self, out: &mut Vec<Output>) {
        // A database behind the window cannot be given the records it
        // lacks: it takes a state transfer first (`opened`).
        if self.lead.serving || self.applied < self.log.first() {
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
    /// heartbeat, it sends again what it is waiting on an answer to; a
    /// replica recovering its RECOVERY, and nothing else.
    pub(super) fn tick(&mut self, out: &mut Vec<Output>) {
        self.timers.beat = self.timers.beat.saturating_add(1);
        let beat = self.timers.beat >= self.options.heartbeat_ticks;
        if beat {
            self.timers.beat = 0;
            self.asked = None;
        }
        if self.status == Status::Recovering {
            if beat {
                self.send_recovery(out);
            }
            return;
        }
        if matches!(self.status, Status::Leaving | Status::Removed) {
            if beat && self.status == Status::Leaving {
                self.ask_started(out);
            }
            return;
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
            // \[DESIGN\] A replica behind its epoch, its log not reaching the
            // mark the epoch began at, starts no view change, nor takes
            // part in one (`view_change.rs`, `notice`): it asks every other
            // member for the state, as any normal in the view answers [VRR
            // §5.2], and learns a later view from them.
            if self.behind_epoch() {
                self.timers.heard = 0;
                self.asked = None;
                self.source = None;
                return self.get_state_from_all(self.commit, out);
            }
            return self.start_view_change(self.view.saturating_add(1), out);
        }
        if beat {
            self.send_again(out);
        }
    }

    /// What a replica waiting on others sends again at a heartbeat.
    fn send_again(&mut self, out: &mut Vec<Output>) {
        match self.status {
            Status::Normal | Status::Recovering | Status::Leaving | Status::Removed => {}
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
            // A backup heard nothing from holds the log the view began
            // with, as STARTVIEW gave it: the records after are sent
            // again, as a PREPARE lost is never sent otherwise.
            let acked = self.lead.acked.get(&backup).copied().unwrap_or(self.lead.inherited);
            // Behind what is retained, a backup learns the commit-number and
            // asks for the state (`transfer.rs`).
            if acked >= self.op() || acked < self.log.first() {
                self.send(backup, &Message::Commit(Commit { view: self.view, commit: self.commit }), out);
                continue;
            }
            let last = acked.saturating_add(self.options.resend_records).min(self.op());
            for op in acked.saturating_add(1)..=last {
                if let Some(record) = self.record(op) {
                    let prepare =
                        Prepare { view: self.view, record: record.clone(), op, commit: self.commit, mark: self.log.mark(op).cloned() };
                    self.send(backup, &Message::Prepare(prepare), out);
                }
            }
        }
    }

    /// The primary of a view begins to serve, its database to append, once
    /// the log it took over is committed and delivered; not again in an
    /// epoch it reconfigured out of (`vr/reconfigure.rs`).
    pub(super) fn serve(&mut self, out: &mut Vec<Output>) {
        let inherited = self.lead.inherited;
        let ready = self.commit >= inherited && self.applied >= inherited && !self.reopening;
        if self.status == Status::Normal && self.is_primary() && !self.lead.serving && ready && !self.reconfiguring() {
            self.lead.serving = true;
            out.push(Output::Role(Role::Primary { view: self.view }));
        }
    }

    /// A serving primary leaves its service: its records not committed are
    /// in doubt, and its database, deposed, opens again from its last
    /// checkpoint; its driver then tells where it holds the log
    /// (`opened`).
    pub(super) fn depose(&mut self, out: &mut Vec<Output>) {
        if self.lead.serving {
            self.lead.serving = false;
            // Not served again until the database, opened again, tells
            // where it holds the log: what it held before is forgotten.
            self.reopening = true;
            out.push(Output::Role(Role::Backup));
            self.in_doubt.append(&mut self.own);
        }
    }

    /// \[EXTENSION\] Its database, opened from its files, holds the log
    /// through `at`, every record of it committed (RULES.md D57): delivery
    /// resumes after it, of what is committed and durable here. [VRR §5.1]
    /// has a recovering replica's application state tell the log prefix
    /// "captured on disk", "so that we obtain all the requests after that
    /// point, and so that we avoid rerunning operations that ran before the
    /// node failed"; its journal intact, it tells only itself. The view it
    /// is in is reported, for the operator, who learns of a view entered
    /// by a change as it happens and of the first, or the one a restart
    /// resumes, here.
    pub(super) fn opened(&mut self, at: u64, out: &mut Vec<Output>) {
        self.delivered = at;
        self.applied = at;
        self.reopening = false;
        if self.status == Status::Recovering {
            return self.begin_recovery(out);
        }
        if at > self.op() {
            // \[DESIGN\] The database holds more than the log: a state
            // transfer it took whose journal writes a crash lost. What
            // it holds is committed, so the log is let go through it, and
            // the log after it fetched (RULES.md D57).
            self.log.retain_after(at);
            out.push(Output::Retain { through: Lsn(at) });
            self.commit = at;
            self.durable = at;
            self.unwritten = true;
            if self.status == Status::Normal {
                self.status = Status::Transfer;
                self.source = None;
            }
        }
        // What the database holds is committed: known so here, as the
        // journal tells only what was let go.
        self.commit = self.commit.max(at.min(self.op()));
        if self.unwritten {
            self.persist_state(out);
        }
        out.push(Output::Epoch { epoch: self.epoch, configuration: self.configuration.clone() });
        if self.status == Status::Removed {
            // Removed before, as its state says: its database, opened again,
            // is retired again.
            out.push(Output::Retire);
            out.push(Output::Event(ReplicaEvent::Removed));
            return;
        }
        out.push(Output::Event(ReplicaEvent::ViewChanged { view: self.view, primary: self.primary_of(self.view) }));
        if at < self.log.first() {
            // \[DESIGN\] The database holds less than the window begins at:
            // the journal was retained past it, by a state transfer whose
            // import a crash lost, or a checkpoint whose files it lost, so
            // the records between are gone from here; it fetches the state
            // of the primary of its view, or of another member if that is
            // this replica, before it takes part (RULES.md D57).
            let primary = self.primary_of(self.view);
            let from = if primary == self.me { self.others().into_iter().next() } else { Some(primary) };
            if let Some(from) = from {
                return self.fetch_state_from(from, out);
            }
        }
        if self.status == Status::Transfer {
            self.get_state(self.commit, out);
        }
        self.deliver(out);
        if self.status == Status::ViewChange && self.is_primary() && self.normal_view == self.view {
            // The primary of its view, made again from its journal: it
            // waits for the next view, which it starts at once, never
            // completing its own again (RULES.md D57).
            self.start_view_change(self.view.saturating_add(1), out);
        }
    }
}
