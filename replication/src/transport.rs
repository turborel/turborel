//! \[EXTENSION\] The transport, an extension point (RULES.md D57,
//! R-SAFE-3): what moves frames between replicas.

use turborel::error::Result;

use crate::{Configuration, ReplicaId};

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
    /// The members of the group, as the transport links them now.
    fn members(&self) -> Vec<ReplicaId>;
    /// \[EXTENSION\] The group reconfigured to `configuration` [VRR §7]:
    /// the members named are linked, by their addresses, those no longer
    /// named let go. Default: nothing, for a transport whose members are
    /// fixed, as a simulator's.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a member's address is malformed.
    fn reconfigure(&self, _configuration: &Configuration) -> Result<()> {
        Ok(())
    }
}
