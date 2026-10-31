//! \[EXTENSION\] VR's state transfer of the log [VRR §5.2]: "State transfer
//! is used by a node that has gotten behind (but hasn't crashed) to bring
//! itself up-to-date." Here, of a backup that missed records of its own
//! view; the log is never cut short (log retention is a later stage), so
//! the log alone always suffices.

use super::frame::{GetState, Message, NewState};
use super::{Status, Vr};
use crate::Output;

impl Vr {
    /// [VRR §5.2]: "To get the state the replica sends a ⟨GETSTATE v, n′,
    /// i⟩ message to one of the other replicas, where v is its view-number
    /// and n′ is its op-number": here to the primary, which has every
    /// record of the view. Asked once until answered or a heartbeat passes.
    pub(super) fn get_state(&mut self, out: &mut Vec<Output>) {
        let op = self.op();
        if self.asked == Some(op) {
            return;
        }
        self.asked = Some(op);
        let get = Message::GetState(GetState { view: self.view, op, replica: self.me });
        Vr::send(self.primary_of(self.view), &get, out);
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
        let after = usize::try_from(m.op).unwrap_or(usize::MAX);
        let log = self.log.get(after..).unwrap_or_default().to_vec();
        let new = Message::NewState(NewState { view: self.view, after: m.op, log, op: self.op(), commit: self.commit });
        Vr::send(m.replica, &new, out);
    }

    /// [VRR §5.2]: "When replica i receives the NEWSTATE message, it
    /// appends the log in the message to its log and updates its state
    /// using the other information in the message." Records it already
    /// holds are skipped: in one view they are the same.
    pub(super) fn on_new_state(&mut self, m: NewState, out: &mut Vec<Output>) {
        if m.view != self.view || self.status != Status::Normal || self.is_primary() || m.after > self.op() {
            return;
        }
        self.asked = None;
        let mut at = m.after;
        for record in m.log {
            at = at.saturating_add(1);
            if at > self.op() {
                self.log.push(record.clone());
                Vr::persist(at, record, out);
            }
        }
        self.learn_commit(m.commit, out);
    }
}
