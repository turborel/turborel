//! \[EXTENSION\] A database's host plug-ins (RULES.md D229): those
//! PLUGINS.HOST declares, started before it opens, each object of them
//! registered as the host would register its own, and the node stopped
//! when one fails.

use std::sync::Arc;

mod register;

use super::Database;
use super::state::State;
use crate::disk::io::{FileSystem, OsFileSystem};
use crate::error::Result;
use crate::plugin::bridge::hosted::files::PluginFiles;
use crate::plugin::bridge::hosted::trust::PluginKeys;
use crate::plugin::errors::{host_failed, invalid_manifest};
use crate::plugin::hosted::{HostLink, HostPlugin, HostSpec};
use crate::plugin::manifest::{HostDeclared, HostKind};
use crate::settings::PLUGINS_HOST;

impl Database {
    /// \[EXTENSION\] Starts the host plug-ins PLUGINS.HOST declares and
    /// registers their objects, before the database is opened from its
    /// files (RULES.md D229). Those started before are not started again.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T05 to 38T07 if one cannot be started, as
    /// `HostPlugin::start` says; an object's own registration's error.
    pub fn start_host_plugins(&self) -> Result<()> {
        let st = self.snapshot()?;
        let host = Arc::clone(&st.extension_host);
        let started: Vec<String> = host.hosted().iter().map(|p| p.manifest.name.clone()).collect();
        for spec in HostSpec::parse_all(&st.settings.get(PLUGINS_HOST).to_string())? {
            if started.contains(&spec.name) {
                continue;
            }
            let plugin = HostPlugin::start(&spec, &host, &host.stop)?;
            self.change(|st| st.declare_manifest_settings(&plugin.manifest, &plugin.pool, true))?;
            host.add_hosted(Arc::clone(&plugin));
            for d in &plugin.manifest.hosted {
                self.register_hosted(d, plugin.link(d))?;
            }
        }
        Ok(())
    }

    /// \[EXTENSION\] The objects of `kind` the host plug-ins started serve,
    /// each with the way to it, for the kinds a layer beneath takes itself
    /// (replication's, a backup's).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn host_objects(&self, kind: HostKind) -> Result<Vec<(HostDeclared, HostLink)>> {
        let host = Arc::clone(&self.snapshot()?.extension_host);
        Ok(host.hosted().iter().flat_map(|p| p.declared(kind).map(|d| (d.clone(), p.link(d))).collect::<Vec<_>>()).collect())
    }

    /// \[EXTENSION\] The file system the database's files are opened
    /// from (RULES.md D251): a host plug-in's, if one is started, else the
    /// operating system's.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed; SQLSTATE 38T06 if more
    /// than one host plug-in serves a file system.
    pub fn file_system(&self) -> Result<Arc<dyn FileSystem>> {
        let mut served = self.host_objects(HostKind::FileSystem)?;
        if served.len() > 1 {
            return Err(invalid_manifest("PLUGINS.HOST may start one HOST FILE SYSTEM, the one the files are opened from"));
        }
        Ok(match served.pop() {
            Some((_, link)) => Arc::new(PluginFiles::new(link)),
            None => Arc::new(OsFileSystem),
        })
    }

    /// \[EXTENSION\] The keyrings host plug-ins serve (RULES.md D252),
    /// for the encryption that reads its keys from them.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn host_keyrings(&self) -> Result<Vec<PluginKeys>> {
        Ok(self.host_objects(HostKind::Keyring)?.into_iter().map(|(d, link)| PluginKeys::new(&d, link)).collect())
    }

    /// \[EXTENSION\] Why the node stopped, if a host plug-in's failure
    /// stopped it (RULES.md D229).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn host_stopped(&self) -> Result<Option<String>> {
        Ok(self.snapshot_unstopped()?.extension_host.stop.stopped())
    }

    /// \[EXTENSION\] Stops the node for `why`, as a host plug-in's failure
    /// does (RULES.md D229): every later statement fails with SQLSTATE
    /// 38T11, those asked are told, and the database is recovered as it
    /// next opens. A layer beneath the database whose own plug-in died,
    /// replication's driver among them, calls it so.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn stop_node(&self, why: &str) -> Result<()> {
        self.snapshot_unstopped()?.extension_host.stop.stop(why);
        Ok(())
    }

    /// Asks that `tell` be called, with the reason, when a host plug-in's
    /// failure stops the node, as a server ends its process then.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn on_host_stop(&self, tell: crate::plugin::hosted::Told) -> Result<()> {
        self.snapshot_unstopped()?.extension_host.stop.on_stop(tell);
        Ok(())
    }
}

/// `st`, or SQLSTATE 38T11 if a host plug-in's failure stopped the node.
pub(super) fn running(st: Arc<State>) -> Result<Arc<State>> {
    if !st.extension_host.stop.tripped() {
        return Ok(st);
    }
    let why = st.extension_host.stop.stopped().unwrap_or_default();
    Err(host_failed("of PLUGINS.HOST", &why))
}
