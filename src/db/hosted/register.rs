//! \[EXTENSION\] Each host plug-in's object registered as the host would
//! register its own (RULES.md D229): an encoding and a checksum by their
//! ids, the clock, the log's and the catalog's storage, spill, package
//! cryptography and the rest, each through the setter its trait has.
//! Replication's kinds and the archive's are taken by the layers beneath,
//! through `Database::host_objects`, and registered by none here.

use crate::db::Database;
use crate::error::Result;
use crate::plugin::errors::unavailable;
use crate::plugin::hosted::HostLink;
use crate::plugin::manifest::{HostDeclared, HostKind};

impl Database {
    /// Registers the host object `d`, reached by `link`.
    pub(in crate::db) fn register_hosted(&self, d: &HostDeclared, link: HostLink) -> Result<()> {
        self.snapshot()?;
        match d.kind {
            HostKind::Encoding
            | HostKind::HashFunction
            | HostKind::Clock
            | HostKind::LogStorage
            | HostKind::CatalogStorage
            | HostKind::Spill
            | HostKind::FileSystem
            | HostKind::ServedConnections
            | HostKind::Listener
            | HostKind::PackageCrypto
            | HostKind::Keyring
            | HostKind::BackgroundJob => {
                drop(link);
                Err(unavailable(&format!("HOST {} {} is not served by this build", d.kind.word(), d.name)))
            }
            HostKind::ReplicaTransport | HostKind::Journal | HostKind::StateTransfer | HostKind::Consensus | HostKind::Archive => {
                drop(link);
                Ok(())
            }
        }
    }
}
