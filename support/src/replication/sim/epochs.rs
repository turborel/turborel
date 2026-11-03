//! The epochs of a simulated run (RULES.md D57, [VRR §7]): each
//! configuration the replicas told the world they entered, by epoch, the
//! members now, and the reconfigurations asked of primaries, which append
//! no more in the view they asked in.

use std::collections::BTreeMap;

use turborel_replication::{Configuration, ReplicaId};

use super::{Rng, Violation};

/// What the run's replicas told of their epochs.
#[derive(Debug, Default)]
pub struct Epochs {
    /// The members of each epoch entered, as first told.
    known: BTreeMap<u64, Vec<ReplicaId>>,
    /// The primaries, by view, that asked a reconfiguration: they take no
    /// record after it in that view, so none asks them for one; with the
    /// latest epoch as they asked, so a reconfiguration pending is known.
    asked: BTreeMap<(ReplicaId, u64), u64>,
}

impl Epochs {
    /// The members of epoch `epoch`, as told; none if no replica entered it.
    pub fn members_of(&self, epoch: u64) -> Option<&[ReplicaId]> {
        self.known.get(&epoch).map(Vec::as_slice)
    }

    /// The latest epoch entered.
    pub fn latest(&self) -> u64 {
        self.known.keys().next_back().copied().unwrap_or(0)
    }

    /// The members of the latest epoch.
    pub fn members(&self) -> &[ReplicaId] {
        self.known.values().next_back().map_or(&[], Vec::as_slice)
    }

    /// Replica `id` entered `epoch` of `configuration`: whether the epoch
    /// is new to the world, which must agree with what was told of it.
    ///
    /// # Errors
    ///
    /// Another configuration was told of the epoch.
    pub fn entered(&mut self, id: ReplicaId, epoch: u64, configuration: &Configuration) -> Result<bool, Violation> {
        let mut members: Vec<ReplicaId> = configuration.iter().map(|m| m.id).collect();
        members.sort_unstable();
        match self.known.get(&epoch) {
            Some(known) if *known != members => {
                Err(format!("replica {} entered epoch {epoch} of members {members:?}, entered before of {known:?}", id.0))
            }
            Some(_) => Ok(false),
            None => {
                self.known.insert(epoch, members);
                Ok(true)
            }
        }
    }

    /// Whether `primary`, serving `view`, asked a reconfiguration in it.
    pub fn pending(&self, primary: ReplicaId, view: u64) -> bool {
        self.asked.contains_key(&(primary, view))
    }

    /// Whether a reconfiguration asked is not yet done: no epoch was
    /// entered since one was asked. One lost with its primary stays so
    /// until the next is done.
    pub fn pending_any(&self) -> bool {
        self.asked.values().any(|&latest| latest == self.latest())
    }

    /// A reconfiguration for `primary`, serving `view`, to ask, if it asked
    /// none in the view and a spare waits: `spare` joins, and a member at
    /// random leaves; the members then, in order.
    pub fn propose(&mut self, rng: &mut Rng, (primary, view): (ReplicaId, u64), spare: Option<ReplicaId>) -> Option<Vec<ReplicaId>> {
        let spare = spare?;
        if self.pending(primary, view) {
            return None;
        }
        let members = self.members();
        let leaving = members[rng.index(members.len())];
        let mut next: Vec<ReplicaId> = members.iter().copied().filter(|&id| id != leaving).collect();
        next.push(spare);
        next.sort_unstable();
        self.asked.insert((primary, view), self.latest());
        Some(next)
    }
}
