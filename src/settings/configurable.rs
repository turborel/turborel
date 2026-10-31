//! \[DESIGN\] RULES.md D36: the settings a plug-in of any kind declares and
//! is told, one hook for every kind. A plug-in that has settings gives
//! itself as its `Configurable` (`StorageEngine::configurable`,
//! `IndexMethod::configurable`, and so for every kind the database
//! registers); one that gives none has none.

use super::{Definition, Settings, builtin, canonical, misuse};
use crate::error::Result;

/// \[DESIGN\] What a plug-in declares of the database's settings, and how it
/// is told their values (RULES.md D36): never by a configuration of its own.
///
/// Contract (RULES.md R-SAFE-3): `configure` returns soon, never calls back
/// into the database (R-ERR-4), and keeps what it needs of the values,
/// which are a copy, never changed under it.
pub trait Configurable: Send + Sync {
    /// The database's settings it declares, each named under the
    /// plug-in's own name ("ROWSTORE.X"), with its kind, default and
    /// effect: declared as it is registered, a value the database was given
    /// before kept, one declared already kept as it is. Default: none.
    fn settings(&self) -> Vec<Definition> {
        Vec::new()
    }

    /// Takes the database's values of the settings, `settings`, as they are
    /// published: once it is registered, after each change of one, and once
    /// the database is opened, its values replayed. It reads its own
    /// (`Settings::key`, `get`) and takes a change as the effect it
    /// declared says: a setting of `Effect::Start` it keeps as it started,
    /// recording that value (`Settings::mark_started`), so a change shows
    /// as waiting for a restart. Default: nothing.
    fn configure(&self, _settings: &Settings) {}
}

impl Settings {
    /// Declares the settings `hook`, of the plug-in `plugin`, declares
    /// (`Configurable::settings`), those declared already kept as they are:
    /// a plug-in registered again under its name, as an engine opened from
    /// its files replacing the one in memory, declares its own again.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one is not under the plug-in's name, or is named as
    /// a built-in setting is (RULES.md D36); or its default is not of its
    /// kind.
    pub(crate) fn declare_plugin(&mut self, plugin: &str, hook: &dyn Configurable) -> Result<()> {
        let group = format!("{}.", canonical(plugin));
        for definition in hook.settings() {
            let name = canonical(&definition.name);
            if !name.starts_with(&group) || builtin().iter().any(|b| b.name == name) {
                return Err(misuse(&format!("the setting {name} of the plug-in {plugin} is not under its name (RULES.md D36)")));
            }
            if self.key(&name).is_none() {
                self.declare(definition)?;
            }
        }
        Ok(())
    }
}

// Unit tests: the hook's defaults (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::{Configurable, Settings};

    /// A plug-in that gives a hook of none.
    struct Bare;

    impl Configurable for Bare {}

    /// Verifies: [DESIGN] RULES.md D36: a hook by default declares no
    /// setting, and being told the values changes nothing.
    #[test]
    fn a_hook_by_default_declares_nothing() {
        let mut settings = Settings::default();
        let before = settings.list(&super::super::Own::default()).len();
        settings.declare_plugin("bare", &Bare).unwrap();
        Bare.configure(&settings);
        assert_eq!(settings.list(&super::super::Own::default()).len(), before);
    }

    /// Verifies: [DESIGN] RULES.md D36: a log, a diagnostics sink and a
    /// clock that give no name of their own are named by default under no
    /// built-in setting's group, so a setting one declares there is never
    /// taken for the database's; and give no hook by default.
    #[test]
    fn a_log_a_sink_and_a_clock_are_named_apart_from_built_in_groups() {
        use crate::clock::{CLOCK_PLUGIN, Clock, SystemClock};
        use crate::diagnostics::{DIAGNOSTICS_PLUGIN, Diagnostics, NoDiagnostics};
        use crate::log::{LOG_PLUGIN, Log, MemoryLog};
        let (log, sink, clock) = (MemoryLog::new(), NoDiagnostics, SystemClock);
        assert_eq!([log.name(), sink.name(), clock.name()], [LOG_PLUGIN, DIAGNOSTICS_PLUGIN, CLOCK_PLUGIN]);
        assert!(log.configurable().is_none() && sink.configurable().is_none() && clock.configurable().is_none());
        for name in [LOG_PLUGIN, DIAGNOSTICS_PLUGIN, CLOCK_PLUGIN] {
            let group = format!("{}.", super::canonical(name));
            assert!(!super::builtin().iter().any(|b| b.name.starts_with(&group)), "{name}");
        }
    }
}
