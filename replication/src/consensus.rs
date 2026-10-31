//! \[EXTENSION\] The consensus protocol, an extension point (RULES.md D57,
//! R-SAFE-3): a deterministic state machine that orders opaque records.
//! It does no I/O, reads no clock and starts no thread; its driver feeds it
//! [`Input`]s and carries out the [`Output`]s it answers with.

use crate::{Lsn, ReplicaEvent, ReplicaId, Role};

/// \[EXTENSION\] A consensus protocol, as a state machine.
///
/// Contract (checked for every implementation by the contract tests and the
/// simulator of `turborel-test-support`):
/// * **Deterministic.** `step` depends only on the state and the input:
///   two machines made alike and fed the same inputs answer the same
///   outputs.
/// * **Total.** `step` never panics, whatever the input: a frame malformed
///   or from no member is refused with an event, and changes nothing else.
/// * **Positions.** An `Append` taken answers `Assigned(n)` and then
///   `Persist { at: n, .. }` of the same record, `n` one past the last
///   position of the log; one refused answers an event and nothing else.
///   A record has the same position on every replica, and positions
///   follow order (the log contract clause of D57).
/// * **Commit.** `Committed(n)` is answered only while the replica serves
///   as primary, between `Role(Primary { .. })` and `Role(Backup)`, never
///   for a smaller `n` than before, and only once a quorum of replicas
///   holds every record through `n` durably. `Role(Primary { .. })` is
///   answered only once every record the replica's log holds is
///   committed, so it commits them all. A record appended in one
///   service and not committed in it is resolved by one `Outcome`, once
///   the replica knows whether the log keeps it.
/// * **Delivery.** `Deliver { through }` names records committed and
///   durable on this replica, which the driver gives the core's replay in
///   order; the records the replica appended itself as primary are never
///   delivered to it while it serves.
/// * **The journal.** The driver carries out the outputs in order. Each
///   `Persist` and `Truncate` changes its journal of records; `PersistState`
///   writes the protocol's own state, and makes it and every journal output
///   before it durable at once, before the next output is carried out.
///   Each `Persist`, once durable, is answered by one `Persisted(at)`, in
///   the order of the `Persist`s. After a crash the driver remakes the
///   machine from what its journal holds ([`Recovered`]).
/// * **Frames.** `Send` frames may be lost, delayed, duplicated or
///   reordered by the transport: the protocol sends again what it needs.
pub trait Consensus: Send {
    /// Its name, as the operator sees it: `"vr"`.
    fn name(&self) -> &'static str;
    /// Takes one input, and adds to `out` what the driver is to do, in
    /// order.
    fn step(&mut self, input: Input, out: &mut Vec<Output>);
}

/// \[EXTENSION\] What a driver tells a consensus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// The primary's database appended a record.
    Append(Vec<u8>),
    /// A frame arrived from another replica.
    Received {
        /// The replica the transport says sent it.
        from: ReplicaId,
        /// The frame, unread.
        frame: Vec<u8>,
    },
    /// The driver's timer: a tick, at an interval the protocol's options
    /// count in.
    Tick,
    /// The journal made the record at this position durable: one for each
    /// `Persist`, in their order.
    Persisted(Lsn),
    /// The core's replay applied the records through this position.
    Applied(Lsn),
    /// A state transfer installed an export taken at this position.
    Installed(Lsn),
}

/// \[EXTENSION\] What a consensus tells its driver to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// The position of the record just appended: what the log's `append`
    /// returns.
    Assigned(Lsn),
    /// Write `record` to the journal at `at`, the position after its last.
    Persist {
        /// Its position.
        at: Lsn,
        /// The record, opaque.
        record: Vec<u8>,
    },
    /// \[EXTENSION\] Remove the journal's records after `after`; it takes
    /// effect with the `PersistState` that follows it.
    Truncate {
        /// The last position kept.
        after: Lsn,
    },
    /// \[EXTENSION\] Write the protocol's own state (for VR, its view), and
    /// make it and the journal outputs before it durable, at once, before
    /// the next output.
    PersistState(Vec<u8>),
    /// Send `frame` to a replica.
    Send {
        /// The replica.
        to: ReplicaId,
        /// The frame, opaque to the transport.
        frame: Vec<u8>,
    },
    /// Every record through this position is committed: it wakes the log's
    /// `sync(p)` for every `p` up to it.
    Committed(Lsn),
    /// Give the core's replay the records committed through this position
    /// that it has not applied.
    Deliver {
        /// The last position to apply.
        through: Lsn,
    },
    /// Whether the record this replica appended at `at`, in a service now
    /// ended, is kept by the log: it resolves a sync in doubt (RULES.md
    /// D25).
    Outcome {
        /// Its position.
        at: Lsn,
        /// Whether the log kept it.
        kept: bool,
    },
    /// Serve as primary, or stop.
    Role(Role),
    /// Fetch a state transfer from a replica: the log no longer reaches
    /// back far enough.
    FetchState {
        /// The replica to fetch it from.
        from: ReplicaId,
    },
    /// Report to the operator.
    Event(ReplicaEvent),
}

/// \[EXTENSION\] What a replica's journal held after a crash, from which
/// its consensus is made again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovered {
    /// The last `PersistState` written, if any.
    pub state: Option<Vec<u8>>,
    /// The records, from position 1 on.
    pub records: Vec<Vec<u8>>,
}
