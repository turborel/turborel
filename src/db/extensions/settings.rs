//! \[EXTENSION\] The settings of an extension's or a host plug-in's
//! manifest made settings of the database's (RULES.md D280, D281):
//! declared as the extension is created or the host plug-in started,
//! changed by the new manifest as it is updated, and retired as it is
//! dropped, under RESTRICT as under CASCADE, as nothing depends on a
//! setting.

use super::super::State;
use crate::error::Result;
use crate::plugin::Manifest;
use crate::plugin::errors::invalid_manifest;
use crate::settings::Owner;

impl State {
    /// Declares the settings `manifest` declares, of a host plug-in if
    /// `host`, and retires those its plug-in declared before and no more.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if one is not of its type, or is named as another
    /// setting is.
    pub(in crate::db) fn declare_manifest_settings(&mut self, manifest: &Manifest, host: bool) -> Result<()> {
        let definitions = manifest.settings.iter().map(|d| d.definition(&manifest.name, host)).collect::<Result<Vec<_>>>()?;
        let owner = Owner { plugin: manifest.name.clone(), host };
        self.settings.declare_owned(&owner, definitions).map_err(|e| invalid_manifest(&e.msg))
    }

    /// Retires the settings the extension `name` declared, as it is dropped.
    pub(super) fn retire_manifest_settings(&mut self, name: &str) {
        self.settings.retire_owned(name);
    }
}

/// What a call of a session's statement against `st` tells the programs
/// of extensions it calls: `st`'s values, its session's own over the
/// database's (RULES.md D282).
pub(in crate::db) fn calling(st: &std::sync::Arc<State>) -> crate::plugin::settings::Calling {
    let st = std::sync::Arc::clone(st);
    crate::plugin::settings::Calling::enter(std::sync::Arc::new(move |name: &str| {
        let settings = &st.settings;
        settings.key(name).map(|k| settings.get(k).clone())
    }))
}
