//! \[EXTENSION\] The settings plug-ins' manifests declare (RULES.md D280,
//! D281): declared as an extension is created or a host plug-in started,
//! each named under its plug-in's name; changed by its new manifest as an
//! extension is updated; and retired as it is dropped.
//!
//! A key is a setting's place, held by a session's own values and by
//! what reads a setting often, so a setting retired keeps its place,
//! unnamed: its name no longer finds it, no listing shows it, no image
//! keeps its value, and one declared again of the same name takes a place
//! of its own. A setting is live while its name finds its place.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Definition, Key, Settings, accepted, canonical, misuse};
use crate::error::Result;

/// Whose a setting a plug-in declared is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// The plug-in's name, as its manifest gives it.
    pub plugin: String,
    /// Whether it is a host plug-in's, which the host gives and SQL may
    /// not set (RULES.md D281).
    pub host: bool,
}

/// The owners of the settings plug-ins declared, by the settings' names.
pub(super) type Owners = Arc<HashMap<String, Owner>>;

impl Settings {
    /// Whether the setting at `at` is live: its name finds it.
    pub(super) fn live(&self, at: usize) -> bool {
        self.definitions.get(at).is_some_and(|d| self.places.get(&d.name) == Some(&at))
    }

    /// The plug-in that declared the setting `name`, if one did through
    /// its manifest.
    pub fn owner(&self, name: &str) -> Option<&Owner> {
        self.owners.get(&canonical(name))
    }

    /// Whether the setting `name` is one the host gives and SQL may not
    /// set: of `HOST_ONLY`, or a host plug-in's (RULES.md D173, D281).
    pub fn host_only(&self, name: &str) -> bool {
        super::HOST_ONLY.contains(&canonical(name).as_str()) || self.owner(name).is_some_and(|o| o.host)
    }

    /// Declares the settings `definitions` the manifest of `owner`
    /// declares, each under its name, or, of one live already, its
    /// definition changed in place, its value kept if still of its kind;
    /// then retires the settings `owner` declared before and declares no
    /// more.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one is not under its plug-in's name, or is named
    /// as another's setting is.
    pub(crate) fn declare_owned(&mut self, owner: &Owner, definitions: Vec<Definition>) -> Result<()> {
        let group = format!("{}.", canonical(&owner.plugin));
        let names: Vec<String> = definitions.iter().map(|d| canonical(&d.name)).collect();
        for (definition, name) in definitions.into_iter().zip(&names) {
            if !name.starts_with(&group) {
                return Err(misuse(&format!("the setting {name} of the plug-in {} is not under its name (RULES.md D280)", owner.plugin)));
            }
            let ours = self.owner(name) == Some(owner);
            match (self.key(name), ours) {
                (Some(key), true) => self.redefine(key, definition),
                (Some(_), false) => return Err(misuse(&format!("{name} is a setting already, not of the plug-in {}", owner.plugin))),
                (None, _) => {
                    self.declare(definition)?;
                }
            }
            Arc::make_mut(&mut self.owners).insert(name.clone(), owner.clone());
        }
        let gone: Vec<String> = self.owners.iter().filter(|(n, o)| *o == owner && !names.contains(n)).map(|(n, _)| n.clone()).collect();
        gone.iter().for_each(|n| self.retire(n));
        Ok(())
    }

    /// Retires every setting `plugin` declared, as it is dropped.
    pub(crate) fn retire_owned(&mut self, plugin: &str) {
        let gone: Vec<String> = self.owners.iter().filter(|(_, o)| o.plugin.eq_ignore_ascii_case(plugin)).map(|(n, _)| n.clone()).collect();
        gone.iter().for_each(|n| self.retire(n));
    }

    /// The setting `name` retired: unnamed, its value gone.
    fn retire(&mut self, name: &str) {
        if let Some(at) = Arc::make_mut(&mut self.places).remove(name)
            && let Some(slot) = self.values.get_mut(at)
        {
            *slot = None;
        }
        self.pending.retain(|(n, _)| n != name);
        Arc::make_mut(&mut self.owners).remove(name);
    }

    /// `definition` in place of the setting at `key`, its value kept if it
    /// is of its new kind.
    fn redefine(&mut self, key: Key, mut definition: Definition) {
        definition.name = canonical(&definition.name);
        let kept = self.values.get(key.0).cloned().flatten().and_then(|v| accepted(&definition.name, &definition.kind, &v).ok());
        if let Some(slot) = self.values.get_mut(key.0) {
            *slot = kept;
        }
        if let Some(d) = Arc::make_mut(&mut self.definitions).get_mut(key.0) {
            *d = definition;
        }
    }
}
