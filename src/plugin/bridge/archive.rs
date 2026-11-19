//! \[EXTENSION\] An archive served by a host plug-in (kind 36, RULES.md
//! D274): a backup target's provider of the scheme its declaration names,
//! made of the node's configuration, so a location of that scheme is
//! reached before any database opens, as a restore needs. Its calls are a
//! backup target's (`backup.rs`). A process that fails stops the node, as
//! any host plug-in's does, but a program's own ERROR fails only its
//! call: a target out of reach fails the backup, the archive round or the
//! restore asked, as a built-in target's does, and leaves nothing a
//! restore would take.

use std::sync::Arc;

use crate::error::SqlError;
use crate::plugin::errors::{ENDED, TIMED_OUT, TOO_LONG, VIOLATION, host_failed};
use crate::plugin::hosted::{HostLink, Stop};
use crate::plugin::payload::replication::ARCHIVE;

use super::Link;
use super::backup::PluginTargets;

/// The node's stop, and the plug-in's name, which a process failing trips.
#[derive(Clone)]
pub struct Stopping {
    stop: Arc<Stop>,
    plugin: String,
}

impl std::fmt::Debug for Stopping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.plugin)
    }
}

impl Stopping {
    /// `e`, a call's failure: a process's failing trips the stop, and is
    /// SQLSTATE 38T11; a program's ERROR is returned as it is.
    pub fn failed(&self, link: &Link, e: SqlError) -> SqlError {
        let process = e.state.is_some_and(|s| [ENDED, TIMED_OUT, VIOLATION, TOO_LONG].contains(&s));
        if !process {
            return e;
        }
        self.stop.stop(&format!("{}.{}: {}", self.plugin, link.object, e.msg));
        host_failed(&self.plugin, &e.msg)
    }
}

/// \[EXTENSION\] The archive `link` names, a provider of targets of the
/// scheme its object's name is.
pub fn archive_targets(link: &HostLink) -> PluginTargets {
    let plugin = link.plugin();
    let to = Link { pool: Arc::clone(&plugin.pool), kind: ARCHIVE, object: link.object().to_owned() };
    PluginTargets::stopping(to, Stopping { stop: Arc::clone(&plugin.stop), plugin: plugin.manifest.name.clone() })
}
