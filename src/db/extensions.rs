//! \[EXTENSION\] Extensions from other parties as objects of a database
//! (RULES.md D168-D171): CREATE EXTENSION, ALTER EXTENSION ... UPDATE and
//! DROP EXTENSION, made on a state as every definition is, logged with
//! the manifest read and remade by recovery from it.
//!
//! An extension's objects are registered in the state that holds it
//! (`objects.rs`), so a transaction that created one and rolled back
//! leaves no trace, and a state dropped stops the extension's processes.
//! Its name is the database's, not a schema's, and its lock is a name of
//! its own (`extension_lock`). What a session does before the statement
//! is made, finding the package, checking it and creating what it
//! requires, is `session.rs`'s; the views of extensions are in `system/extensions.rs`.

mod host;
mod objects;
mod session;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::ddl::Dropped;
use super::definitions::Defined;
use super::{State, rule};
use crate::ast::{ExtensionDefinition, ExtensionRead, ExtensionStatement};
use crate::catalog::{ExtensionDesc, QualifiedName, owner_of, sole};
use crate::error::{Result, SqlError};
use crate::plugin::package::semver::{Range, Version};
use crate::plugin::{Installed, Manifest};
use crate::txn::LockMode;

/// The schema an extension's types are resolved in: none, as they are
/// the same in every schema.
const RESOLVED_IN: &str = "";
/// The schema of the names extensions' locks take: no schema's, as it
/// begins with a space.
const LOCKS: &str = " EXTENSIONS";

/// The lock of the extension `name`.
pub(super) fn extension_lock(name: &str) -> QualifiedName {
    QualifiedName { auth: LOCKS.to_owned(), ident: name.to_owned() }
}

/// A refusal of the rule `detail` breaks.
fn refused(detail: impl std::fmt::Display) -> SqlError {
    rule(format!("[EXTENSION] {detail}"))
}

impl State {
    /// Makes `e`.
    ///
    /// # Errors
    ///
    /// As `create_extension` and `update_extension`.
    pub(super) fn extension_statement(&mut self, e: &ExtensionStatement) -> Result<Defined> {
        match e {
            ExtensionStatement::Create(d) => self.create_extension(d),
            ExtensionStatement::Update(d) => self.update_extension(d),
        }
    }

    /// CREATE EXTENSION `d`, as read.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it was not read, one of its name is created, or its
    /// objects' names are taken; SQLSTATE 38T06 of its manifest; 38T09 if
    /// an extension it requires is not created at a version it takes.
    fn create_extension(&mut self, d: &ExtensionDefinition) -> Result<Defined> {
        let read = read_of(d)?;
        if self.extensions.contains_key(&d.name) {
            return Err(refused(format!("extension {} is created already", d.name)));
        }
        let manifest = manifest_of(d, read)?;
        let mut locks = Vec::new();
        for (name, range) in &manifest.requires {
            self.required(name, range)?;
            locks.push((extension_lock(name), LockMode::Shared));
        }
        let installed = Arc::new(self.installed(manifest, read)?);
        self.register_extension(&installed)?;
        self.put_extension(&installed, read, !read.dependency);
        Ok(Defined { locks, ..Defined::default() })
    }

    /// ALTER EXTENSION `d` UPDATE, as read: its objects made again of its
    /// new manifest. The scripts of its upgrade run after, as the session's
    /// statements (`session.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it was not read or is not created; SQLSTATE 38T09
    /// if an extension requiring it does not take its new version.
    fn update_extension(&mut self, d: &ExtensionDefinition) -> Result<Defined> {
        let read = read_of(d)?;
        let old = self.extensions.get(&d.name).cloned().ok_or_else(|| refused(format!("there is no extension {}", d.name)))?;
        let manifest = manifest_of(d, read)?;
        let version = Version::parse(&manifest.version)?;
        for other in self.extensions.values().filter(|e| e.manifest.name != d.name) {
            for (_, range) in other.manifest.requires.iter().filter(|(n, _)| *n == d.name) {
                if !Range::parse(range)?.matches(&version) {
                    let msg = format!("extension {} requires {} {range}, and {version} is not in it", other.manifest.name, d.name);
                    return Err(crate::plugin::package::errors::conflict(&msg));
                }
            }
        }
        for (name, range) in &manifest.requires {
            self.required(name, range)?;
        }
        self.unregister_extension(&old);
        let explicit = self.catalog.extensions().iter().find(|e| e.name == d.name).is_none_or(|e| e.explicit);
        let installed = Arc::new(self.installed(manifest, read)?);
        self.register_extension(&installed)?;
        self.put_extension(&installed, read, explicit);
        Ok(Defined::default())
    }

    /// That the extension `name` is created at a version `range` takes.
    fn required(&self, name: &str, range: &str) -> Result<()> {
        let found = self.extensions.get(name).ok_or_else(|| {
            crate::plugin::package::errors::conflict(&format!("it requires extension {name} {range}, which is not created"))
        })?;
        let version = Version::parse(&found.manifest.version)?;
        if Range::parse(range)?.matches(&version) {
            return Ok(());
        }
        Err(crate::plugin::package::errors::conflict(&format!("it requires extension {name} {range}, and {version} is created")))
    }

    /// The extension `manifest` declares, in `read`'s directory. Its types
    /// are data types or types beyond the standard, the same in every
    /// schema, so they are resolved alike by a session and as the database
    /// opens.
    fn installed(&self, manifest: Manifest, read: &ExtensionRead) -> Result<Installed> {
        let owner = owner_of(&*self.catalog, RESOLVED_IN).to_owned();
        let resolve = |t: &crate::ast::TypeName| self.data_type_of(t, (RESOLVED_IN, sole(&*self.catalog, &owner)));
        let mut installed = Installed::new(manifest, PathBuf::from(&read.directory), &self.extension_host, &resolve)?;
        installed.signer.clone_from(&read.signer);
        Ok(installed)
    }

    /// `installed` recorded: in the catalog, in place of any of its name,
    /// and among the state's extensions.
    fn put_extension(&mut self, installed: &Arc<Installed>, read: &ExtensionRead, explicit: bool) {
        let m = &installed.manifest;
        let desc = ExtensionDesc {
            name: m.name.clone(),
            version: m.version.clone(),
            directory: read.directory.clone(),
            manifest: m.text.clone(),
            signer: read.signer.clone(),
            digest: read.digest.clone(),
            registry: read.registry.clone(),
            explicit,
        };
        let mut all: Vec<ExtensionDesc> = self.catalog.extensions().iter().filter(|e| e.name != m.name).cloned().collect();
        all.push(desc);
        let mut catalog = self.catalog.duplicate();
        catalog.set_extensions(all);
        self.catalog = Arc::from(catalog);
        let mut map = BTreeMap::clone(&self.extensions);
        map.insert(m.name.clone(), Arc::clone(installed));
        self.extensions = Arc::new(map);
    }

    /// The extensions that require `name`.
    fn requiring(&self, name: &str) -> Vec<String> {
        let requires = |e: &Installed| e.manifest.requires.iter().any(|(n, _)| n == name);
        self.extensions.values().filter(|e| requires(e)).map(|e| e.manifest.name.clone()).collect()
    }

    /// DROP EXTENSION of the lock `qn`: with CASCADE, the extensions that
    /// require it and what binds only with its objects, dropped too.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such extension, or, not `cascade`,
    /// another requires it or something binds only with its objects.
    pub(super) fn drop_extension(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let name = &qn.ident;
        let installed = self.extensions.get(name).cloned().ok_or_else(|| refused(format!("there is no extension {name}")))?;
        let requiring = self.requiring(name);
        let mut dropped = Dropped::default();
        if let (Some(first), false) = (requiring.first(), cascade) {
            return Err(refused(format!("extension {first} requires extension {name}, so it is dropped only with CASCADE")));
        }
        for other in &requiring {
            dropped.absorb(self.drop_extension(&extension_lock(other), cascade)?);
        }
        let mut after = self.clone();
        after.forget_extension(&installed);
        let mut abandoned = self.abandoned_in(&after, &|_| true);
        if !cascade && !abandoned.is_empty() {
            let msg = format!("{} binds only with extension {name}, so it is dropped only with CASCADE", abandoned.first());
            return Err(refused(msg));
        }
        abandoned.columns.clear();
        let (gone, locks) = self.destroy(Arc::clone(&after.catalog), &abandoned)?;
        dropped.absorb(gone);
        dropped.names.extend(locks.into_iter().map(|(n, _): (QualifiedName, LockMode)| n));
        self.forget_extension(&installed);
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// This state without the extension `name`, if it has one: what binds
    /// no more there binds only with it.
    pub(in crate::db) fn without_extension(&self, name: &str) -> State {
        let mut after = self.clone();
        if let Some(installed) = self.extensions.get(name).cloned() {
            after.forget_extension(&installed);
        }
        after
    }

    /// `installed` gone: its objects, its descriptor and its place.
    fn forget_extension(&mut self, installed: &Installed) {
        self.unregister_extension(installed);
        let name = &installed.manifest.name;
        let kept: Vec<ExtensionDesc> = self.catalog.extensions().iter().filter(|e| e.name != *name).cloned().collect();
        let mut catalog = self.catalog.duplicate();
        catalog.set_extensions(kept);
        self.catalog = Arc::from(catalog);
        let mut map = BTreeMap::clone(&self.extensions);
        map.remove(name);
        self.extensions = Arc::new(map);
    }

    /// Each extension of the catalog made, as the database opens from an
    /// image of it: its objects registered, no process started.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if a manifest kept no longer reads.
    pub(in crate::db) fn attach_extensions(&mut self) -> Result<()> {
        let descs: Vec<ExtensionDesc> = self.catalog.extensions().to_vec();
        let mut map = BTreeMap::new();
        for d in descs {
            let read = ExtensionRead {
                manifest: d.manifest.clone(),
                directory: d.directory.clone(),
                signer: d.signer.clone(),
                digest: d.digest.clone(),
                registry: d.registry.clone(),
                dependency: !d.explicit,
            };
            let installed = Arc::new(self.installed(Manifest::parse(&d.manifest)?, &read)?);
            self.register_extension(&installed)?;
            map.insert(d.name.clone(), installed);
            self.extensions = Arc::new(map.clone());
        }
        Ok(())
    }
}

/// What the database read of `d`.
fn read_of(d: &ExtensionDefinition) -> Result<&ExtensionRead> {
    d.read.as_ref().ok_or_else(|| refused(format!("extension {} was not read", d.name)))
}

/// `read`'s manifest, of `d`'s name.
fn manifest_of(d: &ExtensionDefinition, read: &ExtensionRead) -> Result<Manifest> {
    let manifest = Manifest::parse(&read.manifest)?;
    if manifest.name != d.name {
        return Err(refused(format!("the manifest is of extension {}, not {}", manifest.name, d.name)));
    }
    Ok(manifest)
}
