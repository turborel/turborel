//! The keyrings and host plug-ins of `turboreld` (RULES.md D26, D229,
//! D252): the keys of `--keys`, or a host plug-in's keyring once PLUGINS.HOST
//! starts one; the host plug-ins started before the database opens; and
//! the server stopped when one fails.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use tokio::sync::Notify;
use turborel::db::Database;
use turborel::files::Noted;
use turborel::plugin::bridge::hosted::trust::PluginKeys;
use turborel_aes::{KEY_BYTES, KeyDirectory, Keyring};

use crate::{Config, config_file};

/// The host plug-ins' keyrings, once they are started.
pub(crate) type HostKeys = Arc<OnceLock<Vec<PluginKeys>>>;

/// The keyring of `config`: a host plug-in's, once one is started, else
/// the keys of `--keys`, else none; and where the host plug-ins' go.
///
/// # Errors
///
/// The directory of `--keys` cannot be shown among `db`'s files.
pub(crate) fn keyring(db: &Database, config: &Config) -> Result<(Arc<dyn Keyring>, HostKeys), String> {
    let own: Arc<dyn Keyring> = match &config.keys {
        Some(dir) => Arc::new(NotedKeys { keys: KeyDirectory::new(dir), noted: noted(db, dir, false)? }),
        None => Arc::new(NoKeys),
    };
    let hosted = HostKeys::default();
    Ok((Arc::new(Keys { hosted: Arc::clone(&hosted), own }), hosted))
}

/// Starts the host plug-ins PLUGINS.HOST names, as set in `db` or, if
/// `config_given`, as `config` gives it, and gives their keyrings to
/// `keys`.
///
/// # Errors
///
/// One cannot be started, or PLUGINS.HOST cannot be set.
pub(crate) fn start(db: &Database, config: &Config, keys: &HostKeys, config_given: bool) -> Result<(), String> {
    if config_given {
        let entries = config_file::entries(config)?;
        let given = config_file::given(db, config, entries)?;
        given.iter().filter(|g| g.name.trim().eq_ignore_ascii_case("PLUGINS.HOST")).try_for_each(|g| g.set(db))?;
    }
    db.start_host_plugins().map_err(|e| format!("cannot start the host plug-ins: {e}"))?;
    let rings = db.host_keyrings().map_err(|e| e.to_string())?;
    if !rings.is_empty() {
        // Set once: the program starts its host plug-ins once.
        drop(keys.set(rings));
    }
    Ok(())
}

/// Done once a host plug-in's failure stops the node (RULES.md D229).
pub(crate) fn halted(db: &Database) -> impl Future<Output = ()> + use<> {
    let told = Arc::new(Notify::new());
    let telling = Arc::clone(&told);
    // A database closed has no node to stop.
    drop(db.on_host_stop(Arc::new(move |_| telling.notify_one())));
    if db.host_stopped().ok().flatten().is_some() {
        told.notify_one();
    }
    async move { told.notified().await }
}

/// Why the node stopped, for the operator, if a host plug-in stopped it.
pub(crate) fn why_halted(db: &Database) -> Option<String> {
    db.host_stopped().ok().flatten().map(|why| format!("stopped: a host plug-in failed: {why}"))
}

/// The file or directory at `path`, the server's, shown among `db`'s files
/// (RULES.md D56), held open now if `open`.
pub(crate) fn noted(db: &Database, path: &Path, open: bool) -> Result<Noted, String> {
    db.note_file(path, open).map_err(|e| format!("cannot note {}: {e}", path.display()))
}

/// The keyring encryption reads: the first host plug-in's, if one was
/// started, else the server's own.
struct Keys {
    hosted: HostKeys,
    own: Arc<dyn Keyring>,
}

impl Keyring for Keys {
    fn name(&self) -> &'static str {
        self.hosted.get().and_then(|r| r.first()).map_or_else(|| self.own.name(), PluginKeys::name)
    }
    fn key(&self, name: &str) -> turborel::error::Result<Option<[u8; KEY_BYTES]>> {
        match self.hosted.get().and_then(|r| r.first()) {
            Some(ring) => ring.key(name),
            None => self.own.key(name),
        }
    }
}

/// The keyring of `--keys`, its directory shown among the database's files,
/// each key read counted.
struct NotedKeys {
    keys: KeyDirectory,
    noted: Noted,
}

impl Keyring for NotedKeys {
    fn name(&self) -> &'static str {
        self.keys.name()
    }
    fn key(&self, name: &str) -> turborel::error::Result<Option<[u8; KEY_BYTES]>> {
        self.noted.read();
        self.keys.key(name)
    }
}

/// The keyring of a server given no `--keys`: it has none, and says how
/// to give them.
struct NoKeys;

impl Keyring for NoKeys {
    fn name(&self) -> &'static str {
        "none: give turboreld --keys DIR"
    }
    fn key(&self, _: &str) -> turborel::error::Result<Option<[u8; KEY_BYTES]>> {
        Ok(None)
    }
}
