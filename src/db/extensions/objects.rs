//! \[EXTENSION\] An extension's objects registered in a state (RULES.md
//! D169): its functions, aggregates, table functions and procedures in
//! the state's registry of functions, which each state copies; its
//! metrics and models in the shared registries, held weakly, so they are
//! found while a state holds the extension; its authenticators, targets
//! and strategies found through the state's extensions. A name taken by
//! a built-in, registered or other extension's object is refused.
//!
//! Of the kinds of RULES.md D220: rankings and quantizers are in the
//! shared registries, held weakly; collations, system tables and XQuery
//! functions in the state's own, which each state copies; credentials
//! and sinks of events are found through the state's extensions, and at
//! most one extension brings credentials (D222). A language is named in
//! the whole process (D328), held weakly, so it goes as its extension.

use std::sync::Arc;

use super::super::State;
use super::refused;
use crate::error::Result;
use crate::fulltext::rank::Ranking;
use crate::fulltext::text::served::ServedLanguage;
use crate::plugin::bridge::language::PluginLanguage;
use crate::plugin::bridge::xquery::PluginXQuery;
use crate::plugin::{Bridged, Declared, Installed, ObjectKind};
use crate::system::SystemTable;
use crate::value::collation::Collation;
use crate::vector::quantizer::Quantizer;
use crate::xquery::XQueryFunction;

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
                Bridged::Authenticator(_) | Bridged::Target(_) | Bridged::Diagnostics(_) | Bridged::Rule(_) => {
                    self.unshared(installed, d.kind, &d.name, false)?;
                }
                Bridged::Ranking(_)
                | Bridged::Quantizer(_)
                | Bridged::Collation(_)
                | Bridged::SystemTable(_)
                | Bridged::XQuery(_)
                | Bridged::Credentials(_)
                | Bridged::Type(_)
                | Bridged::Engine(_)
                | Bridged::IndexMethod(_)
                | Bridged::Language(_) => self.register_object(installed, d, b)?,
            }
        }
        Ok(())
    }

    /// Registers `b`, of a kind of RULES.md D220, which `d` declares.
    fn register_object(&mut self, installed: &Installed, d: &Declared, b: &Bridged) -> Result<()> {
        match b {
            Bridged::Ranking(r) => {
                self.unshared(installed, d.kind, &d.name, self.rankings.registered(&d.name))?;
                let r = Arc::clone(r);
                let r: Arc<dyn Ranking> = r;
                self.rankings.add_extension(&r);
            }
            Bridged::Quantizer(q) => {
                self.unshared(installed, d.kind, &d.name, self.vectors.registered_quantizer(&d.name))?;
                let q = Arc::clone(q);
                let q: Arc<dyn Quantizer> = q;
                self.vectors.add_extension_quantizer(&q);
            }
            Bridged::Collation(c) => {
                self.unshared(installed, d.kind, &d.name, self.collations.contains_key(&d.name))?;
                let c = Arc::clone(c);
                let c: Arc<dyn Collation> = c;
                self.collations.insert(d.name.clone(), c);
            }
            Bridged::SystemTable(t) => {
                let name = crate::system::name_of(&**t);
                let taken = self.system.contains_key(&name) || crate::system::view(&name).is_some();
                self.unshared(installed, d.kind, &d.name, taken)?;
                let t = Arc::clone(t);
                let t: Arc<dyn SystemTable> = t;
                self.system.insert(name, t);
            }
            Bridged::XQuery(f) => self.xquery_functions.add(Arc::<PluginXQuery>::clone(f))?,
            Bridged::Credentials(_) => self.only_credentials(installed)?,
            Bridged::Type(t) => {
                self.unshared(installed, d.kind, &d.name, false)?;
                self.types.add_kind(t.kind())?;
            }
            Bridged::Engine(_) | Bridged::IndexMethod(_) => self.register_storage(installed, d, b)?,
            Bridged::Language(l) => self.register_language(installed, d, l)?,
            Bridged::Scalar(_)
            | Bridged::Aggregate(_)
            | Bridged::Table(_)
            | Bridged::Procedure(_)
            | Bridged::Metric(_)
            | Bridged::Embedding(_)
            | Bridged::Partition(_)
            | Bridged::Authenticator(_)
            | Bridged::Target(_)
            | Bridged::Diagnostics(_)
            | Bridged::Rule(_) => {}
        }
        Ok(())
    }

    /// Registers `l`, the language `d` of `installed` declares, in the
    /// whole process (RULES.md D328).
    fn register_language(&self, installed: &Installed, d: &Declared, l: &Arc<PluginLanguage>) -> Result<()> {
        self.unshared(installed, d.kind, &d.name, false)?;
        let l = Arc::clone(l);
        let l: Arc<dyn ServedLanguage> = l;
        crate::fulltext::text::served::register(&d.name, &installed.manifest.name, &l).map(drop)
    }

    /// That no other extension brings the database's credentials.
    fn only_credentials(&self, installed: &Installed) -> Result<()> {
        let others = self.extensions.values().filter(|e| e.manifest.name != installed.manifest.name);
        if let Some(other) = others.into_iter().find(|e| e.objects.iter().any(|(o, _)| o.kind == ObjectKind::Credentials)) {
            return Err(refused(format!("extension {} brings the database's credentials already", other.manifest.name)));
        }
        Ok(())
    }

    /// Registers `b`, a storage engine or an index method, which `d`
    /// declares: its own registration again no clash.
    fn register_storage(&mut self, installed: &Installed, d: &Declared, b: &Bridged) -> Result<()> {
        if let Bridged::Engine(e) = b {
            let ours = self.engines.get(&d.name).is_some_and(|held| std::ptr::addr_eq(Arc::as_ptr(held), Arc::as_ptr(e)));
            self.unshared(installed, d.kind, &d.name, self.engines.contains_key(&d.name) && !ours)?;
            let e = Arc::clone(e);
            let e: Arc<dyn crate::storage::StorageEngine> = e;
            self.engines.insert(d.name.clone(), e);
        } else if let Bridged::IndexMethod(m) = b {
            let ours = self.methods.get(&d.name).is_some_and(|held| std::ptr::addr_eq(Arc::as_ptr(held), Arc::as_ptr(m)));
            self.unshared(installed, d.kind, &d.name, self.methods.contains_key(&d.name) && !ours)?;
            let m = Arc::clone(m);
            let m: Arc<dyn crate::index::IndexMethod> = m;
            self.methods.insert(d.name.clone(), m);
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
                Bridged::Collation(c) => {
                    let ours = self.collations.get(c.name()).is_some_and(|held| std::ptr::addr_eq(Arc::as_ptr(held), Arc::as_ptr(c)));
                    if ours {
                        self.collations.remove(c.name());
                    }
                }
                Bridged::SystemTable(t) => {
                    let name = crate::system::name_of(&**t);
                    let ours = self.system.get(&name).is_some_and(|held| std::ptr::addr_eq(Arc::as_ptr(held), Arc::as_ptr(t)));
                    if ours {
                        self.system.remove(&name);
                    }
                }
                Bridged::XQuery(f) => {
                    let f = Arc::clone(f);
                    let f: Arc<dyn XQueryFunction> = f;
                    self.xquery_functions.remove(&f);
                }
                Bridged::Type(t) => self.types.remove_kind(t.kind()),
                Bridged::IndexMethod(m) => {
                    let name = crate::index::IndexMethod::name(&**m);
                    if self.methods.get(name).is_some_and(|held| std::ptr::addr_eq(Arc::as_ptr(held), Arc::as_ptr(m))) {
                        self.methods.remove(name);
                    }
                }
                Bridged::Engine(e) => {
                    let name = crate::storage::StorageEngine::name(&**e);
                    if self.engines.get(name).is_some_and(|held| std::ptr::addr_eq(Arc::as_ptr(held), Arc::as_ptr(e))) {
                        self.engines.remove(name);
                    }
                }
                Bridged::Metric(_)
                | Bridged::Embedding(_)
                | Bridged::Authenticator(_)
                | Bridged::Target(_)
                | Bridged::Partition(_)
                | Bridged::Ranking(_)
                | Bridged::Quantizer(_)
                | Bridged::Credentials(_)
                | Bridged::Diagnostics(_)
                | Bridged::Rule(_)
                | Bridged::Language(_) => {}
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

    /// \[EXTENSION\] The credentials an extension brings, if one does
    /// (RULES.md D222).
    pub(in crate::db) fn extension_credentials(&self) -> Option<Arc<dyn crate::db::Credentials>> {
        let mut all = self.extensions.values().flat_map(|e| e.objects.iter());
        let found = all.find_map(|(_, b)| if let Bridged::Credentials(c) = b { Some(Arc::clone(c)) } else { None })?;
        let found: Arc<dyn crate::db::Credentials> = found;
        Some(found)
    }

    /// \[EXTENSION\] The sinks of events extensions bring.
    pub(in crate::db) fn extension_sinks(&self) -> Vec<Arc<crate::plugin::bridge::process::PluginDiagnostics>> {
        let all = self.extensions.values().flat_map(|e| e.objects.iter());
        all.filter_map(|(_, b)| if let Bridged::Diagnostics(d) = b { Some(Arc::clone(d)) } else { None }).collect()
    }
}
