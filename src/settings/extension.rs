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

// Unit tests: settings declared, changed and retired by their owner
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::super::{Definition, Effect, Kind, Own, Settings};
    use super::Owner;
    use crate::value::{Decimal, Value};

    /// A database's whole-number setting `name` of `max`.
    fn integer(name: &str, max: i128) -> Definition {
        let kind = Kind::Integer { min: 0, max };
        Definition { name: name.into(), kind, default: Value::Exact(Decimal::new(1, 0)), per_session: false, node: false, effect: Effect::Statement, doc: String::new() }
    }

    /// Verifies: [EXTENSION] RULES.md D280: a plug-in's settings declared
    /// again keep their places and the database's values still of their
    /// kinds; those it no longer declares are retired, unnamed, unlisted,
    /// in no image, their values gone; none is declared outside its name
    /// or over another's; and one retired declared again takes a place of
    /// its own at its default.
    #[test]
    fn owned_settings_are_redeclared_and_retired() {
        let mut s = Settings::default();
        let owner = Owner { plugin: "E".into(), host: false };
        s.declare_owned(&owner, vec![integer("E.A", 10), integer("E.B", 10)]).unwrap();
        let (a, b) = (s.key("e.a").unwrap(), s.key("E.B").unwrap());
        s.set("E.A", Some(&Value::Exact(Decimal::new(7, 0)))).unwrap();
        s.set("E.B", Some(&Value::Exact(Decimal::new(9, 0)))).unwrap();
        s.declare_owned(&owner, vec![integer("E.A", 10), integer("E.B", 5)]).unwrap();
        assert_eq!((s.key("E.A"), s.integer(a), s.integer(b)), (Some(a), 7, 1), "B's 9 is no longer of its kind");
        s.declare_owned(&owner, vec![integer("E.A", 10)]).unwrap();
        assert!(s.key("E.B").is_none() && s.owner("E.B").is_none());
        assert!(!s.list(&Own::default()).iter().any(|l| l.definition.name == "E.B"));
        s.retire_owned("e");
        assert!(s.key("E.A").is_none() && !s.database_values().iter().any(|(n, _)| n.starts_with("E.")));
        assert!(s.declare_owned(&owner, vec![integer("F.A", 10)]).is_err());
        let other = Owner { plugin: "SESSION".into(), host: true };
        assert!(s.declare_owned(&other, vec![integer("SESSION.TIME_ZONE", 1)]).is_err());
        s.declare_owned(&owner, vec![integer("E.A", 10)]).unwrap();
        assert_ne!(s.key("E.A"), Some(a));
        assert_eq!(s.integer(s.key("E.A").unwrap()), 1);
        assert!(!s.host_only("E.A") && s.host_only("plugins.host"));
    }
}
