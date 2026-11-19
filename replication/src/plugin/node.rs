//! \[EXTENSION\] A node's replication served by its host plug-ins
//! (RULES.md D270): the objects of kinds 32 to 35 that PLUGINS.HOST's
//! plug-ins declare, at most one of each kind, each on a process of its
//! own (`Hosted`), in place of the built-in transport, journal, state
//! transfer and consensus. A kind no plug-in serves stays built in.

use std::path::Path;
use std::sync::Arc;

use turborel::db::Database;
use turborel::disk::io::OsFileSystem;
use turborel::error::{Code, Result, SqlError};
use turborel::plugin::hosted::HostLink;
use turborel::plugin::manifest::HostKind;
use turborel::plugin::payload::replication::{CONSENSUS, JOURNAL, REPLICA_TRANSPORT, STATE_TRANSFER};

use super::{Hosted, PluginConsensus, PluginJournal, PluginTransfer, PluginTransport};
use crate::driver::{Make, OpenJournal};
use crate::{Journal, Member, ReplicaId, nonce, staging_dir};

/// \[EXTENSION\] The parts of a node its host plug-ins serve, each none if
/// none does.
#[derive(Default)]
pub struct Plugged {
    /// Its transport.
    pub transport: Option<Arc<PluginTransport>>,
    /// Its state transfer.
    pub transfer: Option<Arc<PluginTransfer>>,
    /// What opens its journal.
    pub journal: Option<OpenJournal>,
    /// The way to its consensus, opened by `make` as each is made.
    pub consensus: Option<HostLink>,
}

impl Plugged {
    /// Whether a host plug-in serves any of them: the node then stops as
    /// its driver fails (`Driver::on_halt`).
    pub fn any(&self) -> bool {
        self.transport.is_some() || self.transfer.is_some() || self.journal.is_some() || self.consensus.is_some()
    }

    /// What makes the node's consensus: the plug-in's, opened on a process
    /// of its own each time one is made, else `built_in`.
    pub fn make(&self, built_in: Make) -> Make {
        let Some(link) = self.consensus.clone() else { return built_in };
        Arc::new(move |configuration: &[Member], me: ReplicaId, recovered: Option<crate::Recovered>| {
            let pipe = Hosted::new(link.clone(), CONSENSUS)?;
            let made = PluginConsensus::open(Box::new(pipe), (configuration, me), recovered.as_ref(), nonce())?;
            let boxed: Box<dyn crate::Consensus> = Box::new(made);
            Ok(boxed)
        })
    }
}

/// \[EXTENSION\] The parts of replica `me` of `configuration`, its
/// database kept in `dir`, that `db`'s host plug-ins serve, the plug-ins
/// started first.
///
/// # Errors
///
/// SQLCODE -910 if two objects of one kind are declared; a plug-in's,
/// starting or opening.
pub fn plugged(db: &Database, dir: &Path, (configuration, me): (&[Member], ReplicaId)) -> Result<Plugged> {
    db.start_host_plugins()?;
    let transport = match one(db, HostKind::ReplicaTransport)? {
        Some(link) => Some(Arc::new(PluginTransport::open(Box::new(Hosted::new(link, REPLICA_TRANSPORT)?), me, &configuration.to_vec())?)),
        None => None,
    };
    let transfer = match one(db, HostKind::StateTransfer)? {
        Some(link) => {
            Some(Arc::new(PluginTransfer::new(Box::new(Hosted::new(link, STATE_TRANSFER)?), Arc::new(OsFileSystem), staging_dir(dir))))
        }
        None => None,
    };
    let journal = one(db, HostKind::Journal)?.map(|link| {
        let open: OpenJournal = Arc::new(move || {
            let (journal, recovered) = PluginJournal::open(Box::new(Hosted::new(link.clone(), JOURNAL)?))?;
            let boxed: Box<dyn Journal> = Box::new(journal);
            Ok((boxed, recovered))
        });
        open
    });
    Ok(Plugged { transport, transfer, journal, consensus: one(db, HostKind::Consensus)? })
}

/// The one object of `kind` the host plug-ins declare, if one does.
fn one(db: &Database, kind: HostKind) -> Result<Option<HostLink>> {
    let mut found = db.host_objects(kind)?;
    if found.len() > 1 {
        return Err(SqlError::new(
            Code::Api,
            format!(
                "[EXTENSION] RULES.md D270: PLUGINS.HOST declares {} objects of HOST {}, and a node takes one",
                found.len(),
                kind.word()
            ),
        ));
    }
    Ok(found.pop().map(|(_, link)| link))
}
