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
//! * [`Journal`]: where a replica keeps what its consensus has it persist;
//!   [`LogJournal`], kept in a core `Log`, is the first.
//!
//! [`ReplicatedLog`] is a core `Log` over a consensus and a journal: of one
//! replica for now, which drives its consensus itself, and as the log of a
//! database kept in a directory, [`ReplicatedLogs`]. [`Driver`] joins a
//! replica of a group: its consensus, journal and transport and the core's
//! `Standby` seam of its database, which writes to the driver's log
//! ([`GroupLog`]); a job of the thread manager (D55, [`DriverJob`]) runs
//! its turns.
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
pub mod driver;
mod journal;
mod log;
mod storage;
mod transfer;
mod transport;
pub mod vr;

pub use consensus::{Consensus, Input, Output, Recovered};
pub use driver::{Driver, DriverJob, GroupLog, GroupLogs, Joined, Make, Parts};
pub use journal::{Journal, LogJournal};
pub use log::{MakeAlone, ReplicatedLog, TICKS_ALONE};
pub use storage::{ALONE, ReplicatedLogs, STORAGE_NAME, vr_alone};
pub use transfer::{ANSWER, BLOCK, Fetching, PIECE, PageDifferences, REQUEST, StateTransfer, WholeFiles, staging_dir};
pub use transport::Transport;
pub use turborel::log::Lsn;
pub use vr::{Vr, VrOptions};

/// \[EXTENSION\] A replica: its number among the members of a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplicaId(pub u32);

/// \[EXTENSION\] A member of a group's configuration (RULES.md D57,
/// reconfiguration): its number, and where its node takes frames, as
/// REPLICATION.MEMBERS names it, `host:port`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Member {
    /// Its number, never given to another node of the group.
    pub id: ReplicaId,
    /// Where its node takes the others' frames.
    pub address: String,
}

/// \[EXTENSION\] A group's configuration: its members, by number [VRR §7].
pub type Configuration = Vec<Member>;

/// \[EXTENSION\] A reconfiguration marked on a record of the log, the
/// last of its epoch: the epoch it begins, and that epoch's members [VRR
/// §7].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// The epoch the reconfiguration begins.
    pub epoch: u64,
    /// Its members.
    pub configuration: Configuration,
}

/// \[EXTENSION\] What a replica's database is to do, as consensus decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Serve as primary in `view`: take changes, whose records it appends
    /// (the core's `Standby::promote`).
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
    /// \[EXTENSION\] The replica's journal held no state, lost or never
    /// written: it recovers from the group before it takes part [VRR
    /// §4.3], and tells the view it then enters.
    Recovering,
    /// \[EXTENSION\] The replica recovered: normal in `view`, with the log
    /// of `from`, that view's primary, or its own, a new group's.
    Recovered {
        /// The view it is normal in.
        view: u64,
        /// Whose log it holds.
        from: ReplicaId,
    },
    /// \[EXTENSION\] The replica entered `epoch`, whose members are
    /// `configuration` [VRR §7]: as it opens, and at each reconfiguration
    /// committed.
    Reconfigured {
        /// The epoch.
        epoch: u64,
        /// Its members.
        configuration: Configuration,
    },
    /// \[EXTENSION\] The replica is no member of the epoch entered, and the
    /// new members hold the state: it takes no further part, its database
    /// retired [VRR §7].
    Removed,
}

/// \[EXTENSION\] A number no earlier start of a replica had, from the
/// clock: the nonce of a recovery (`Vr::restart`).
pub fn nonce() -> u64 {
    transfer::stamp()
}
