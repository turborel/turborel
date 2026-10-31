//! \[EXTENSION\] The transport, an extension point (RULES.md D57,
//! R-SAFE-3): what moves frames between replicas.

use turborel::error::Result;

use crate::ReplicaId;

/// \[EXTENSION\] Moves frames between the replicas of a group. Built in
/// later: TCP with mutual TLS in the server; in tests, the simulator's
/// network (`turborel-test-support`).
///
/// Contract (checked for every implementation by the contract tests of
/// `turborel-test-support`):
/// * `members` lists every replica of the group, this one included, in
///   order, each once.
/// * `send` to a member returns without waiting for the frame to arrive;
///   the frame may be lost, delayed, duplicated or reordered, and the
///   consensus sends again what it needs. It arrives, if it does, with the
///   bytes sent, credited to this replica.
/// * `send` to a replica that is no member is refused (SQLCODE -910).
/// * Methods may be called from several threads at once.
pub trait Transport: Send + Sync {
    /// Sends `frame` to `to`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `to` is no member; -919 if the transport is closed.
    fn send(&self, to: ReplicaId, frame: &[u8]) -> Result<()>;
    /// The members of the group.
    fn members(&self) -> &[ReplicaId];
}
