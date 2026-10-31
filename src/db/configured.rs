//! \[DESIGN\] RULES.md D36: the plug-ins of every kind as they declare the
//! database's settings and are told their values, through one hook
//! (`Configurable`): each declares its own under its name as it is
//! registered, and is told the values then, after each change, and once
//! the database is opened, its values replayed. Told from a snapshot, no
//! lock held, so none calls back to read them (RULES.md R-ERR-4).

use super::{Database, State};
use crate::error::Result;
use crate::settings::{Configurable, Settings};

/// Tells `hook`, if a plug-in has one, the values `settings`.
fn tell(hook: Option<&dyn Configurable>, settings: &Settings) {
    if let Some(hook) = hook {
        hook.configure(settings);
    }
}

impl State {
    /// Declares the settings `hook` of the plug-in `name` declares, if it
    /// has one (`Settings::declare_plugin`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a setting is not under its name, or its default is
    /// not of its kind.
    pub(super) fn declare_plugin(&mut self, name: &str, hook: Option<&dyn Configurable>) -> Result<()> {
        hook.map_or(Ok(()), |hook| self.settings.declare_plugin(name, hook))
    }

    /// Tells every plug-in the state holds its settings' values.
    fn tell_plugins(&self) {
        let settings = &self.settings;
        self.engines.values().for_each(|e| tell(e.configurable(), settings));
        self.methods.values().for_each(|m| tell(m.configurable(), settings));
        self.functions.configurables().for_each(|f| tell(Some(f), settings));
        self.system.values().for_each(|t| tell(t.configurable(), settings));
        self.hashes.iter().for_each(|h| tell(h.configurable(), settings));
        self.encodings.all().unwrap_or_default().iter().for_each(|e| tell(e.configurable(), settings));
        self.types.configurables().for_each(|k| tell(Some(k), settings));
        tell(self.work.spill().configurable(), settings);
    }
}

impl Database {
    /// Declares the settings of the plug-in `name`, `hook`, if it has one,
    /// for a plug-in kept apart from the definitions (`State`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a setting is not under its name, or its default is
    /// not of its kind; if called back from a plug-in.
    pub(super) fn declare_plugin(&self, name: &str, hook: Option<&dyn Configurable>) -> Result<()> {
        match hook {
            Some(hook) => self.change(|st| st.declare_plugin(name, Some(hook))),
            None => Ok(()),
        }
    }

    /// Tells `hook`, of a plug-in just registered, the database's values.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub(super) fn tell_plugin(&self, hook: Option<&dyn Configurable>) -> Result<()> {
        tell(hook, &self.snapshot()?.settings);
        Ok(())
    }

    /// \[DESIGN\] Tells every plug-in registered the database's values of
    /// the settings as they are now published (`Configurable::configure`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub(super) fn configure_plugins(&self) -> Result<()> {
        let st = self.snapshot()?;
        st.tell_plugins();
        let settings = &st.settings;
        tell(self.catalog_storage().configurable(), settings);
        if let Some(log) = self.log_storage_set() {
            tell(log.configurable(), settings);
        }
        if let Some(credentials) = self.credentials() {
            tell(credentials.configurable(), settings);
        }
        if let Some(served) = self.served_connections() {
            tell(served.configurable(), settings);
        }
        Ok(())
    }
}
