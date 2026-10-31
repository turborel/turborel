//! \[EXTENSION\] Replication for turborel (RULES.md D57): a database's log,
//! catalog and durable files kept on several replicas, as opaque bytes.
//!
//! Replication orders, stores and moves bytes it never decodes: what a
//! record means, and how it is applied, is the core's one replay. Its own
//! extension points are traits, each a contract its implementations are
//! tested against (R-SAFE-3):
//!
//! * [`Consensus`]: a protocol that orders records, as a deterministic state
//!   machine, with no I/O, clock or thread of its own; [`Vr`], Viewstamped
//!   Replication, is the first.
//! * [`Transport`]: what moves frames between replicas.
//! * [`StateTransfer`]: how an export of a database crosses to a replica
//!   too far behind for the log alone.
//!
//! A driver, a job of the thread manager (D55), joins them to a journal of
//! records and to the core; it is built in a later stage, and is not here.
//!
//! # Sources
//!
//! Protocol steps are quoted from, and cited as:
//!
//! ```text
//! [VRR §4.1]  B. Liskov, J. Cowling, "Viewstamped Replication Revisited",
//!             MIT-CSAIL-TR-2012-021, 2012, section 4.1
//! [VR88]      B. Oki, B. Liskov, "Viewstamped Replication: A New Primary
//!             Copy Method to Support Highly-Available Distributed
//!             Systems", PODC 1988, pp. 8-17
//! ```
//!
//! Where turborel departs from \[VRR\], the code says so, with the reason.

mod consensus;
mod transfer;
mod transport;
pub mod vr;

pub use consensus::{Consensus, Input, Output, Recovered};
pub use transfer::StateTransfer;
pub use transport::Transport;
pub use turborel::log::Lsn;
pub use vr::{Vr, VrOptions};

/// \[EXTENSION\] A replica: its number among the members of a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplicaId(pub u32);

/// \[EXTENSION\] What a replica's database is to do, as consensus decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Serve as primary in `view`: take changes, whose records it appends
    /// (`Database::promote` in the core, a later stage).
    Primary {
        /// The view it is primary of.
        view: u64,
    },
    /// Stop serving as primary: its records not yet committed are in doubt,
    /// each resolved by an [`Output::Outcome`] (the core's depose).
    Backup,
}

/// \[EXTENSION\] What a consensus reports for the operator, beside what the
/// driver acts on (diagnostics, RULES.md D40).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicaEvent {
    /// The replica entered `view`, in which `primary` orders records.
    ViewChanged {
        /// The view.
        view: u64,
        /// Its primary.
        primary: ReplicaId,
    },
    /// A frame was refused, unread: malformed, or from no member.
    FrameRefused {
        /// The replica it came from, as the transport says.
        from: ReplicaId,
        /// Why, as the error's message.
        why: String,
    },
    /// A record was appended while this replica serves as no primary: the
    /// driver appends only between `Role(Primary)` and `Role(Backup)`.
    AppendRefused,
}
