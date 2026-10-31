//! \[EXTENSION\] State transfer, an extension point (RULES.md D57,
//! R-SAFE-3): how an export of a database crosses to a replica the log
//! alone can no longer bring up to date ([VRR §5.2]: "Should a gap occur,
//! the slow replica first brings itself almost up to date using
//! application state … to get to a recent checkpoint, and then completes
//! the job by obtaining the log forward from the point").

use turborel::db::StateSource;
use turborel::error::Result;

use crate::{ReplicaId, Transport};

/// \[EXTENSION\] Moves an export of a database between replicas, never
/// decoding it. Built in later: whole files; then page differences, as
/// [VRR §5.1]'s Merkle trees.
///
/// Contract (for the contract tests of every implementation, from its
/// first):
/// * What `receive` returns gives the same image, the same files and the
///   same bytes of each as the source `serve` sent.
/// * `serve` sends only through `tx`, and only to `to`.
/// * Frames lost, damaged or from another transfer are refused (SQLCODE
///   -919 or -916), never installed: `receive` fails, and the receiving
///   replica asks again.
pub trait StateTransfer: Send + Sync {
    /// Sends `source` to `to` over `tx`.
    ///
    /// # Errors
    ///
    /// The source's errors reading, or the transport's sending.
    fn serve(&self, source: Box<dyn StateSource>, to: ReplicaId, tx: &dyn Transport) -> Result<()>;
    /// Makes an export again from the frames `from` sent.
    ///
    /// # Errors
    ///
    /// SQLCODE -919 or -916 if the frames are not a whole transfer.
    fn receive(&self, from: ReplicaId, frames: &mut dyn Iterator<Item = Vec<u8>>) -> Result<Box<dyn StateSource>>;
}
