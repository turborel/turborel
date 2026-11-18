//! Library modules [XQuery §4.2] and module imports [XQuery §4.11]: the
//! Module Feature [XQuery §5.2.5]. A library module is a registered XQuery
//! module (`LibraryModule`), analysed as it is registered
//! (`library_module`) and again, in its own static context, within each
//! query that imports it, into that query's plan (`analyze/linked.rs`).
//!
//! \[DESIGN\] Implementation-defined, RULES.md D106: a module import
//! imports one module of its target namespace [XQuery §4.11], of those
//! the importing module may import: of the main module, the registered
//! XQuery modules the current user may use; of a library module, those it
//! imported as it was registered, so a library module imports what it was
//! analysed with, whoever imports it. Of these, the one whose location, or
//! whose name, alone or qualified by its schema's, is a location hint, else
//! the one of the least name; a library module importing its own target
//! namespace imports another of it. A module is analysed once in a query,
//! however many modules import it. As a module imports only modules
//! registered before it, the graph of imports has no cycle, so
//! err:XQST0073 guards a catalog no statement makes.

use std::sync::Arc;

use super::super::ast::{Module, ModuleBody, ModuleImport};
use super::super::error::{ErrorCode, error};
use super::super::types::Defined;
use super::linked::{Exports, Linked};
use super::scope::display;
use super::{Analysis, Environment, InScopeSchema, Own};
use crate::error::Result;
use crate::xml::Version;

/// [XQuery §4.11] A registered XQuery module, as a module import finds it:
/// a library module, and what it was analysed with as it was registered.
#[derive(Debug, Clone)]
pub struct LibraryModule {
    /// Its name, as the catalog writes a qualified name.
    pub name: String,
    /// Its name's identifier, without its schema's.
    pub ident: String,
    /// Its location URI, the zero-length string if it has none.
    pub location: String,
    /// Its text, parsed: a library module.
    pub module: Arc<Module>,
    /// The names of the registered XQuery modules its module imports
    /// imported as it was registered.
    pub modules: Vec<String>,
    /// The registered XML Schemas its schema imports may import [XQuery
    /// §4.10], of those it kept as it was registered.
    pub importable: Vec<InScopeSchema>,
    /// Its in-scope schema definitions [ISO/IEC 9075-14:2006 draft §10.20
    /// SR2e], of those it kept as it was registered.
    pub schemas: Vec<InScopeSchema>,
}

impl LibraryModule {
    /// Its target namespace [XQuery §4.2]; none of a text of no module
    /// declaration, which no registered XQuery module has.
    #[must_use]
    pub fn namespace(&self) -> Option<&str> {
        match &self.module.body {
            ModuleBody::Library(decl, _) => Some(&decl.uri),
            ModuleBody::Main(..) => None,
        }
    }

    /// Whether one of `hints`, the location hints of a module import, is
    /// its location, or its name, qualified or not.
    fn hinted(&self, hints: &[String]) -> bool {
        hints.iter().any(|h| *h == self.location || *h == self.name || *h == self.ident)
    }
}

/// [XQuery §4.2] A library module analysed as it is registered: its text
/// parsed, its target namespace, and what its imports imported.
#[derive(Debug)]
pub struct Analysed {
    /// Its text, parsed.
    pub module: Arc<Module>,
    /// Its target namespace.
    pub namespace: String,
    /// The names of the registered XQuery modules its module imports
    /// imported, in the order imported.
    pub modules: Vec<String>,
    /// The names of the registered XML Schemas its schema imports imported,
    /// then, if it has a validate expression, those of its in-scope schema
    /// definitions.
    pub schemas: Vec<String>,
}

/// [XQuery §4.2, §4.11] The library module `text` is, of the name `name`
/// and its identifier `ident`, analysed in its own static context: of
/// `env`'s functions and collations, its schema imports and in-scope
/// schema definitions `env`'s, and its module imports importing among
/// `env`'s modules.
///
/// # Errors
///
/// SQLSTATE 10000, XQuery error, of the static error \[XQuery\] raises of
/// the module or of a module it imports; err:XPST0003 of a main module,
/// as a registered XQuery module is a library module [XQuery §4];
/// SQLCODE -900 past turborel's limits.
pub fn library_module(text: &str, (name, ident): (&str, &str), location: &str, env: &Environment<'_>) -> Result<Analysed> {
    let module = Arc::new(crate::xquery::parse(text, Version::V11)?);
    let ModuleBody::Library(decl, _) = &module.body else {
        let why = "a registered XQuery module is a library module, which begins with a module declaration [XQuery §4]";
        return Err(error(ErrorCode::XPST0003, why));
    };
    let namespace = decl.uri.clone();
    let library = LibraryModule {
        name: name.to_owned(),
        ident: ident.to_owned(),
        location: location.to_owned(),
        module: Arc::clone(&module),
        modules: env.modules.iter().map(|m| m.name.clone()).collect(),
        importable: env.importable.clone(),
        schemas: env.schemas.clone(),
    };
    let mut analysis = Analysis::library(env, &library, Linked::default(), 0);
    analysis.exported()?;
    analysis.linked.circularity()?;
    let modules = analysis.imported_modules.iter().map(|m| m.name.clone()).collect();
    let validated = analysis.own.schemas.iter().filter(|_| analysis.validates);
    let schemas = analysis.imported.iter().chain(validated).map(|s| s.name.clone()).collect();
    Ok(Analysed { module, namespace, modules, schemas })
}

impl<'e> Analysis<'e> {
    /// The analysis of `library`, `depth` imports from the main module,
    /// its declarations to be added to `linked`: its static context its
    /// own [XQuery §4.11], its module declaration's prefix bound to its
    /// target namespace [XQuery §4.2].
    fn library(env: &'e Environment<'e>, library: &'e LibraryModule, linked: Linked, depth: usize) -> Analysis<'e> {
        let known = |n: &String| env.modules.iter().chain(&env.linked).find(|m| m.name == *n);
        let modules = library.modules.iter().filter_map(known).collect();
        let own = Own { library: Some(library), importable: &library.importable, schemas: &library.schemas, modules, depth };
        let mut analysis = Analysis::new(env, own, linked);
        if let ModuleBody::Library(decl, _) = &library.module.body {
            analysis.scope.names.prefixes.push((decl.prefix.as_str().into(), decl.uri.as_str().into()));
        }
        analysis
    }

    /// The library module analysed [XQuery §4.2]: its prolog, then its
    /// static context kept for the evaluation; what it exports.
    fn exported(&mut self) -> Result<Exports> {
        let Some(ModuleBody::Library(_, prolog)) = self.own.library.map(|l| &l.module.body) else { return Ok(Exports::default()) };
        self.setup(&prolog.setup)?;
        let exports = self.declarations(&prolog.declarations)?;
        let context = self.context();
        self.linked.end(self.module, context);
        Ok(exports)
    }

    /// [XQuery §4.11] `import` applied: a registered XQuery module of its
    /// target namespace imported, its prefix bound to that namespace, and
    /// its functions and variables added to the module's function
    /// signatures and in-scope variables.
    ///
    /// # Errors
    ///
    /// err:XQST0047 if the prolog imported the namespace before;
    /// err:XQST0059 if no module the importing module may import has it;
    /// err:XQST0034 or err:XQST0049 if a function or variable imported has
    /// the name, and of a function the arity, of one imported before; and
    /// the static errors of the module imported.
    pub(super) fn import_module(&mut self, import: &ModuleImport) -> Result<()> {
        let uri = import.uri.as_str();
        if self.imported_modules.iter().any(|m| m.namespace() == Some(uri)) {
            let why = format!("the prolog imports the modules of target namespace '{uri}' twice [XQuery §4.11]");
            return Err(error(ErrorCode::XQST0047, &why));
        }
        let importing = self.own.library.map(|l| l.name.as_str());
        let candidates: Vec<&'e LibraryModule> =
            self.own.modules.iter().copied().filter(|m| m.namespace() == Some(uri) && Some(m.name.as_str()) != importing).collect();
        let Some(found) = candidates.iter().copied().find(|m| m.hinted(&import.at)).or_else(|| candidates.first().copied()) else {
            let why = format!("no registered XQuery module the module may import has the target namespace '{uri}' [XQuery §4.11]");
            return Err(error(ErrorCode::XQST0059, &why));
        };
        if let Some(prefix) = &import.prefix {
            self.scope.names.prefixes.push((prefix.as_str().into(), uri.into()));
        }
        let exports = self.link(found)?;
        for (name, arity, place) in exports.functions {
            if self.declared.iter().any(|(n, a, _)| n.same(&name) && *a == arity) {
                let why = format!("the function {} of {arity} parameters is imported twice [XQuery §4.11]", display(&name));
                return Err(error(ErrorCode::XQST0034, &why));
            }
            self.declared.push((name, arity, place));
        }
        for (name, place) in exports.globals {
            if self.globals.iter().any(|(g, _)| g.same(&name)) {
                return Err(error(ErrorCode::XQST0049, &format!("the variable ${} is imported twice [XQuery §4.11]", display(&name))));
            }
            self.globals.push((name, place));
        }
        self.imported_modules.push(found);
        Ok(())
    }

    /// What `found` exports, analysed into the query once.
    ///
    /// # Errors
    ///
    /// As `Linked::open`, and the static errors of `found`.
    fn link(&mut self, found: &'e LibraryModule) -> Result<Exports> {
        if let Some(exports) = self.linked.exports(&found.name) {
            return Ok(exports.clone());
        }
        let depth = self.own.depth.saturating_add(1);
        self.linked.open(&found.name, depth)?;
        let mut analysis = Analysis::library(self.env, found, std::mem::take(&mut self.linked), depth);
        let exports = analysis.exported()?;
        self.linked = analysis.linked;
        self.linked.close(&found.name, exports.clone());
        Ok(exports)
    }

    /// [XQuery §4.11] The error if a module imported declares a function
    /// or variable of a schema type that is none of the module's in-scope
    /// schema types \[err:XQST0036\]: no schema it imports is that type's,
    /// nor defines a type of that name.
    pub(super) fn imported_types(&self) -> Result<()> {
        for module in &self.imported_modules {
            let Some(exports) = self.linked.exports(&module.name) else { continue };
            if let Some(missing) = exports.types.iter().find(|t| !self.in_scope_type(t)) {
                let why = format!(
                    "the module {} declares a function or variable of the type {missing}, none of the in-scope schema types of the module importing it [XQuery §4.11]",
                    module.name
                );
                return Err(error(ErrorCode::XQST0036, &why));
            }
        }
        Ok(())
    }

    /// Whether `t` is one of the in-scope schema types: of a schema the
    /// module imports, or of the name of one.
    fn in_scope_type(&self, t: &Defined) -> bool {
        self.imported.iter().any(|i| Arc::ptr_eq(&i.schema, &t.schema)) || self.imported_type(&t.name).is_some()
    }
}

#[cfg(test)]
mod tests;
