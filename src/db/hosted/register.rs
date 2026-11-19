//! \[EXTENSION\] Each host plug-in's object registered as the host would
//! register its own (RULES.md D229): an encoding and a checksum by their
//! ids, the clock and its ticking job, the log's and the catalog's
//! storage, spill, package cryptography, served connections, jobs and
//! the buffer pool's eviction policy and its reporting job,
//! each through the setter its trait has. A keyring is taken by the
//! encryption that uses it, a file system by the opening of the files, a
//! listener by the server, and replication's kinds and the archive's by
//! the layers beneath, each through `Database::host_objects`.

use std::sync::Arc;
use std::time::Duration;

use crate::db::Database;
use crate::error::Result;
use crate::plugin::bridge::hosted::eviction::PluginEviction;
use crate::plugin::bridge::hosted::page::{PluginEncoding, PluginHash};
use crate::plugin::bridge::hosted::spill::PluginSpill;
use crate::plugin::bridge::hosted::store::{PluginCatalogStorage, PluginLogStorage};
use crate::plugin::bridge::hosted::time::{PluginClock, PluginJob};
use crate::plugin::bridge::hosted::trust::{PluginCrypto, PluginServed};
use crate::plugin::hosted::HostLink;
use crate::plugin::manifest::{HostDeclared, HostKind};

impl Database {
    /// Registers the host object `d`, reached by `link`.
    pub(in crate::db) fn register_hosted(&self, d: &HostDeclared, link: HostLink) -> Result<()> {
        match d.kind {
            HostKind::Encoding => self.register_encoding(Box::new(PluginEncoding::new(d, link)?)),
            HostKind::HashFunction => self.register_hash_function(Box::new(PluginHash::new(d, link)?)),
            HostKind::Clock => {
                let clock = PluginClock::new(d, link)?;
                let read_by: Arc<dyn crate::clock::Clock> = Arc::<PluginClock>::clone(&clock);
                self.set_clock(read_by)?;
                let ticking = self.add_job(clock.ticker())?;
                self.request_job(ticking, Duration::ZERO);
                Ok(())
            }
            HostKind::BufferEviction => {
                let eviction = PluginEviction::new(d, link)?;
                self.set_eviction(eviction.maker())?;
                let advising = self.add_job(eviction.advisor())?;
                // The first policy was asked as it was registered.
                self.request_job(advising, eviction.every());
                Ok(())
            }
            HostKind::PackageCrypto => self.set_package_crypto(Arc::new(PluginCrypto::new(link))),
            HostKind::ServedConnections => self.set_served_connections(Box::new(PluginServed::new(d, link))),
            HostKind::BackgroundJob => {
                let job = self.add_job(Arc::new(PluginJob::new(d, link)))?;
                self.request_job(job, Duration::ZERO);
                Ok(())
            }
            HostKind::LogStorage => self.set_log_storage(Box::new(PluginLogStorage::new(d, link)?)),
            HostKind::CatalogStorage => self.set_catalog_storage(Box::new(PluginCatalogStorage::new(d, link))),
            HostKind::Spill => self.set_spill(Box::new(PluginSpill::new(d, link))),
            HostKind::Keyring
            | HostKind::FileSystem
            | HostKind::Listener
            | HostKind::ReplicaTransport
            | HostKind::Journal
            | HostKind::StateTransfer
            | HostKind::Consensus
            | HostKind::Archive => {
                drop(link);
                Ok(())
            }
        }
    }
}

