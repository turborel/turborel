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
//!   first (`transfer.rs`), and the log after it as before.
//! * A fresh member, with no journal and a database holding nothing,
//!   answers too, saying so, so that a new group, every member fresh,
//!   bootstraps: a fresh member that hears from f fresh members, itself
//!   the f + 1th, is normal in view 0 with an empty log, as [`Vr::new`]
//!   makes a replica of a new group. A group of one recovers at once,
//!   from its database alone. A member that lost its journal is not
//!   fresh: its database holds the log through a position, from which it
//!   follows again, so it waits for a primary.
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
    /// Whether it is fresh: no journal, and a database holding nothing.
    fresh: bool,
    /// The responses, by replica.
    responses: BTreeMap<ReplicaId, RecoveryResponse>,
}

impl Vr {
    /// `Input::Opened` of a replica recovering, its database holding the
    /// log through `at`: its RECOVERY sent; a group of one recovers at
    /// once.
    pub(super) fn begin_recovery(&mut self, at: u64, out: &mut Vec<Output>) {
        self.recovering.fresh = at == 0 && self.op() == 0 && self.log.first() == 0;
        out.push(Output::Event(ReplicaEvent::Recovering));
        self.send_recovery(out);
        self.check_recovered(out);
    }

    /// ⟨RECOVERY i, x⟩ to every other member.
    pub(super) fn send_recovery(&self, out: &mut Vec<Output>) {
        let recovery = Message::Recovery(Recovery { replica: self.me, nonce: self.recovering.nonce });
        self.broadcast(&recovery, out);
    }

    /// A RECOVERY from `m.replica`: answered while normal, as primary with
    /// the log; and while fresh, saying so.
    pub(super) fn on_recovery(&mut self, m: Recovery, out: &mut Vec<Output>) {
        let response = if self.status == Status::Normal {
            let log = self.is_primary().then(|| self.primary_log());
            RecoveryResponse { view: self.view, nonce: m.nonce, replica: self.me, fresh: false, log }
        } else if self.status == Status::Recovering && self.recovering.fresh {
            RecoveryResponse { view: 0, nonce: m.nonce, replica: self.me, fresh: true, log: None }
        } else {
            return;
        };
        Vr::send(m.replica, &Message::RecoveryResponse(response), out);
    }

    /// This primary's log, as a window, for a replica recovering.
    fn primary_log(&self) -> PrimaryLog {
        let after = self.log.first();
        PrimaryLog { after, log: self.log.after(after), op: self.op(), commit: self.commit }
    }

    /// A RECOVERYRESPONSE: kept if it echoes the nonce of this recovery.
    pub(super) fn on_recovery_response(&mut self, m: RecoveryResponse, out: &mut Vec<Output>) {
        if self.status != Status::Recovering || m.nonce != self.recovering.nonce {
            return;
        }
        self.recovering.responses.insert(m.replica, m);
        self.check_recovered(out);
    }

    /// Whether the responses complete the recovery: f + 1 of them, the
    /// primary of the latest view's among them with its log; or, fresh
    /// (or alone), f fresh ones and no other.
    fn check_recovered(&mut self, out: &mut Vec<Output>) {
        let f = self.members.len().saturating_sub(self.quorum());
        let responses = &self.recovering.responses;
        let latest = responses.values().map(|r| r.view).max().unwrap_or(0);
        let primary = self.primary_of(latest);
        if responses.len() > f
            && let Some(log) = responses.get(&primary).and_then(|r| r.log.clone())
        {
            return self.recover_from(latest, log, out);
        }
        let fresh = responses.values().filter(|r| r.fresh).count();
        if (self.recovering.fresh || self.members.len() == 1) && fresh >= f && responses.values().all(|r| r.fresh) {
            self.bootstrap(out);
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
            return self.fetch_state(out);
        }
        let first = self.log.first();
        if self.op() > first {
            out.push(Output::Truncate { after: Lsn(first) });
        }
        if taken.after > first {
            out.push(Output::Retain { through: Lsn(taken.after) });
        }
        self.log = Window::new(taken.after, Vec::new());
        for (index, record) in taken.log.into_iter().enumerate() {
            let at = taken.after.saturating_add(u64::try_from(index).unwrap_or(u64::MAX)).saturating_add(1);
            self.log.push(record.clone());
            self.persist(at, record, out);
        }
        self.commit = taken.commit.max(held).min(self.op());
        self.recovered(out);
        self.acknowledge(out);
        self.after_commit(out);
    }

    /// A new group's member, fresh, or a replica alone: normal in view 0,
    /// its log beginning where its database holds it; its primary serves
    /// once the records it holds are committed and delivered.
    fn bootstrap(&mut self, out: &mut Vec<Output>) {
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
        if self.is_primary() {
            self.try_commit(out);
        }
        self.after_commit(out);
    }

    /// Normal in its view, its state written, the view told.
    fn recovered(&mut self, out: &mut Vec<Output>) {
        self.normal_view = self.view;
        self.status = Status::Normal;
        self.recovering = Recovering::default();
        self.persist_state(out);
        out.push(Output::Event(ReplicaEvent::ViewChanged { view: self.view, primary: self.primary_of(self.view) }));
    }
}
