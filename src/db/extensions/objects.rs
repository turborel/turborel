//! \[EXTENSION\] An extension's objects registered in a state (RULES.md
//! D169): its functions, aggregates, table functions and procedures in
//! the state's registry of functions, which each state copies; its
//! metrics and models in the shared registries, held weakly, so they are
//! found while a state holds the extension; its authenticators, targets
//! and strategies found through the state's extensions. A name taken by
//! a built-in, registered or other extension's object is refused.

use std::sync::Arc;

use super::super::State;
use super::refused;
use crate::error::Result;
use crate::plugin::{Bridged, Installed, ObjectKind};

impl State {
    /// Registers `installed`'s objects.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 or -103 if a name is taken.
    pub(super) fn register_extension(&mut self, installed: &Installed) -> Result<()> {
        for (d, b) in &installed.objects {
            match b {
                Bridged::Scalar(f) => self.functions.add_scalar(Arc::clone(f))?,
                Bridged::Aggregate(f) => {
                    if self.functions.aggregate(&d.name).is_some() || self.functions.scalar(&d.name).is_some() {
                        return Err(refused(format!("a function {} is there already", d.name)));
                    }
                    self.functions.add_aggregate(Arc::clone(f))?;
                }
                Bridged::Table(f) => self.functions.add_table(Arc::clone(f))?,
                Bridged::Procedure(p) => self.functions.add_procedure(Arc::clone(p))?,
                Bridged::Metric(m) => {
                    self.unshared(installed, d.kind, &d.name, self.vectors.registered_metric(&d.name))?;
                    self.vectors.add_extension_metric(m);
                }
                Bridged::Embedding(m) => {
                    self.unshared(installed, d.kind, &d.name, self.embedders.is_registered(&d.name))?;
                    self.embedders.add_extension(m);
                }
                Bridged::Partition(_) => self.unshared(installed, d.kind, &d.name, self.strategies.named(&d.name).is_some())?,
                Bridged::Authenticator(_) | Bridged::Target(_) => self.unshared(installed, d.kind, &d.name, false)?,
            }
        }
        Ok(())
    }

    /// That no other extension has an object of `kind` and `name`, nor is
    /// one `registered`.
    fn unshared(&self, installed: &Installed, kind: ObjectKind, name: &str, registered: bool) -> Result<()> {
        let others = self.extensions.values().filter(|e| e.manifest.name != installed.manifest.name);
        let mut theirs = others.flat_map(|e| e.objects.iter().map(move |(d, _)| (e, d)));
        let taken = theirs.find(|(_, d)| d.kind == kind && d.name.eq_ignore_ascii_case(name)).map(|(e, _)| e.manifest.name.clone());
        if let Some(other) = taken {
            return Err(refused(format!("extension {other} has a {} {name} already", kind.word())));
        }
        if registered {
            return Err(refused(format!("a {} {name} is built in or registered already", kind.word())));
        }
        Ok(())
    }

    /// Takes `installed`'s objects away. Its metrics and models go as the
    /// last state holding it does.
    pub(super) fn unregister_extension(&mut self, installed: &Installed) {
        for (_, b) in &installed.objects {
            match b {
                Bridged::Scalar(f) => self.functions.remove_scalar(f),
                Bridged::Aggregate(f) => self.functions.remove_aggregate(f),
                Bridged::Table(f) => self.functions.remove_table(f),
                Bridged::Procedure(p) => self.functions.remove_procedure(p),
                Bridged::Metric(_) | Bridged::Embedding(_) | Bridged::Authenticator(_) | Bridged::Target(_) | Bridged::Partition(_) => {}
            }
        }
    }

    /// \[EXTENSION\] The object of `kind` named `name` of an extension,
    /// if one has it, as the server and the host find an authenticator, a
    /// backup target's scheme or a partitioning strategy (RULES.md D169).
    pub(in crate::db) fn extension_object(&self, kind: ObjectKind, name: &str) -> Option<Bridged> {
        let mut all = self.extensions.values().flat_map(|e| e.objects.iter());
        all.find(|(d, _)| d.kind == kind && d.name.eq_ignore_ascii_case(name)).map(|(_, b)| b.clone())
    }
}
