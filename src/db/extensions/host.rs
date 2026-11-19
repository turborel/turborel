//! \[DESIGN\] What the host gives a database's extensions and asks of
//! them (RULES.md D170, D173): the settings PLUGINS.* told to the limits
//! and the keys trusted as they change, the cryptography of packages, and
//! the authentication mechanisms extensions bring, which the server finds
//! by name.

use std::sync::Arc;

use super::super::Database;
use crate::error::Result;
use crate::plugin::bridge::auth::PluginAuthenticator;
use crate::plugin::bridge::backup::PluginTargets;
use crate::plugin::bridge::partition::PluginStrategy;
use crate::plugin::limits::Told;
use crate::plugin::package::{PackageCrypto, Trust};
use crate::plugin::{Bridged, ObjectKind};
use crate::settings::{
    PLUGINS_CALL_TIMEOUT_MS, PLUGINS_CANCEL_GRACE_MS, PLUGINS_DIRECTORIES, PLUGINS_MAX_CALL_BYTES, PLUGINS_MAX_MESSAGE_BYTES,
    PLUGINS_MEMORY_BYTES, PLUGINS_PROCESSES, PLUGINS_REQUIRE_SIGNED, PLUGINS_TRUSTED_KEYS, Settings,
};

/// The setting `key`, a count.
fn count(settings: &Settings, key: crate::settings::Key) -> u64 {
    u64::try_from(settings.integer(key)).unwrap_or(0)
}

impl Database {
    /// Tells the extensions' limits and trust the settings PLUGINS.* now.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T08 if PLUGINS.TRUSTED_KEYS names a key that is none,
    /// or a file that cannot be read; SQLCODE -910 if the database is
    /// closed.
    pub(in crate::db) fn tell_extensions(&self) -> Result<()> {
        let st = self.snapshot()?;
        let s = &st.settings;
        let told = Told {
            processes: count(s, PLUGINS_PROCESSES),
            timeout_ms: count(s, PLUGINS_CALL_TIMEOUT_MS),
            max_message: count(s, PLUGINS_MAX_MESSAGE_BYTES),
            max_call: count(s, PLUGINS_MAX_CALL_BYTES),
            memory: count(s, PLUGINS_MEMORY_BYTES),
            grace_ms: count(s, PLUGINS_CANCEL_GRACE_MS),
            directories: s.get(PLUGINS_DIRECTORIES).to_string(),
        };
        let trust = Trust::parse(&s.get(PLUGINS_TRUSTED_KEYS).to_string(), s.is_on(PLUGINS_REQUIRE_SIGNED))?;
        st.extension_host.tell(&told, trust);
        Ok(())
    }

    /// \[EXTENSION\] Registers the cryptography a package's digests and
    /// signature are checked by (RULES.md D173), as the host gives it: the
    /// core makes none of its own (D26). Without it a package file is
    /// refused, and a directory installed from one is not loaded.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn set_package_crypto(&self, crypto: Arc<dyn PackageCrypto>) -> Result<()> {
        self.snapshot()?.extension_host.set_crypto(crypto);
        Ok(())
    }

    /// \[EXTENSION\] The authentication mechanism `mechanism` an extension
    /// brings, if one does, as the server finds one by name (RULES.md D169).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn extension_authenticator(&self, mechanism: &str) -> Result<Option<Arc<PluginAuthenticator>>> {
        Ok(match self.snapshot()?.extension_object(ObjectKind::Authenticator, mechanism) {
            Some(Bridged::Authenticator(a)) => Some(a),
            Some(_) | None => None,
        })
    }

    /// \[EXTENSION\] The backup target's scheme `scheme` an extension
    /// brings, if one does, as a backup finds one by name.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn extension_target(&self, scheme: &str) -> Result<Option<Arc<PluginTargets>>> {
        Ok(match self.snapshot()?.extension_object(ObjectKind::Target, scheme) {
            Some(Bridged::Target(t)) => Some(t),
            Some(_) | None => None,
        })
    }

    /// \[EXTENSION\] The partitioning strategy `name` an extension brings,
    /// if one does, as partitioning finds one by name.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn extension_strategy(&self, name: &str) -> Result<Option<Arc<PluginStrategy>>> {
        Ok(match self.snapshot()?.extension_object(ObjectKind::Partition, name) {
            Some(Bridged::Partition(p)) => Some(p),
            Some(_) | None => None,
        })
    }
}
