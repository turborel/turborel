//! \[EXTENSION\] Where a replica stands, for the operator and for
//! diagnostics. Split from vr.rs (RULES.md R-SIZE-2).

use super::{Status, Vr};
use crate::{Lsn, ReplicaId};

/// \[EXTENSION\] Where a replica stands, for the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    /// The epoch [VRR §7].
    pub epoch: u64,
    /// The view-number.
    pub view: u64,
    /// The status.
    pub status: Status,
    /// The primary of the view.
    pub primary: ReplicaId,
    /// The position before the first record its log holds: those through
    /// it let go by retention.
    pub first: Lsn,
    /// The op-number: the position of its log's last record.
    pub op: Lsn,
    /// The commit-number, as far as it knows.
    pub commit: Lsn,
    /// Through where its journal holds its log durably.
    pub durable: Lsn,
    /// Through where its database said it applied the log.
    pub applied: Lsn,
}

impl Vr {
    /// \[EXTENSION\] Where this replica stands.
    pub fn standing(&self) -> Standing {
        Standing {
            epoch: self.epoch,
            view: self.view,
            status: self.status,
            primary: self.primary_of(self.view),
            first: Lsn(self.log.first()),
            op: Lsn(self.op()),
            commit: Lsn(self.commit),
            durable: Lsn(self.durable),
            applied: Lsn(self.applied),
        }
    }

    /// Where it stands, in words, for diagnostics (`Consensus::describe`).
    pub(super) fn described(&self) -> String {
        format!(
            "{:?}, began {}, durable {}, serving {}, acked {:?}, asked {:?} of {:?}",
            self.standing(),
            self.epoch_began,
            self.durable,
            self.lead.serving,
            self.lead.acked,
            self.asked,
            self.source
        )
    }
}
