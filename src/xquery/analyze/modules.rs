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
//! however many modules import it: its functions and variables are given
//! their places first (`Analysis::reserve`), so a module it imports that
//! imports it back, in a cycle of modules of one namespace [XQuery §4.11],
//! finds them; a cycle of modules not all of one namespace is
//! err:XQST0073 (`Linked::cycle`).

use std::sync::Arc;

use super::super::ast::{Declaration, Import, Module, ModuleBody, ModuleImport, NamespaceKind, SchemaPrefix, Setup};
use super::super::error::{ErrorCode, error};
use super::super::plan::{Body, Function, Global, GlobalValue, Implementation, Op};
use super::super::types::SeqType;
use super::linked::{Exports, Linked, Reserved};
use super::scope::{Scope, Unprefixed, display};
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
    analysis.begin()?;
    analysis.exported()?;
    analysis.linked.circularity()?;
    analysis.linked.checked_types()?;
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

    /// The analysis of the library module begun: its functions and
    /// variables given their places, and it noted as being analysed.
    ///
    /// # Errors
    ///
    /// As `Linked::open`.
    fn begin(&mut self) -> Result<()> {
        let Some(library) = self.own.library else { return Ok(()) };
        let (reserved, exports) = self.reserve();
        self.reserved = reserved;
        self.linked.open((&library.name, library.namespace().unwrap_or_default()), self.own.depth, exports)
    }

    /// Places for the functions and variables the library module's prolog
    /// declares, before it is analysed, each its name resolved by the
    /// prefixes and default function namespace its prolog's first part
    /// declares, which the parser has kept to one binding each; and what
    /// they are as it exports them. A name that does not resolve gets none,
    /// as its analysis refuses it.
    fn reserve(&mut self) -> (Reserved, Exports) {
        let (mut reserved, mut exports) = (Reserved::default(), Exports::default());
        let Some(ModuleBody::Library(_, prolog)) = self.own.library.map(|l| &l.module.body) else { return (reserved, exports) };
        let mut names = self.scope.names.clone();
        for setup in &prolog.setup {
            let bound = match setup {
                Setup::Namespace(decl) => Some((decl.prefix.as_str(), decl.uri.as_str())),
                Setup::Import(Import::Module(i)) => i.prefix.as_deref().map(|p| (p, i.uri.as_str())),
                Setup::Import(Import::Schema(i)) => match &i.prefix {
                    Some(SchemaPrefix::Namespace(p)) => Some((p.as_str(), i.uri.as_str())),
                    Some(SchemaPrefix::DefaultElement) | None => None,
                },
                Setup::DefaultNamespace(decl) => {
                    if decl.kind == NamespaceKind::Function {
                        names.default_function = (!decl.uri.is_empty()).then(|| decl.uri.as_str().into());
                    }
                    None
                }
                Setup::Setter(_) => None,
            };
            names.prefixes.extend(bound.map(|(p, u)| (p.into(), u.into())));
        }
        let scope = Scope::new(names);
        for declaration in &prolog.declarations {
            match declaration {
                Declaration::Function(f) => {
                    let place = scope.name(&f.name, Unprefixed::Function).ok().map(|name| {
                        let arity = f.params.len();
                        let place = self.linked.function(None, self.placeholder(&name, arity));
                        exports.functions.push((name, arity, place));
                        place
                    });
                    reserved.functions.push_back(place);
                }
                Declaration::Variable(v) => {
                    let place = scope.name(&v.name, Unprefixed::None).ok().map(|name| {
                        let global = Global { name: name.clone(), declared: None, value: GlobalValue::Missing, module: self.module };
                        let place = self.linked.global(None, global, None);
                        exports.globals.push((name, place));
                        place
                    });
                    reserved.globals.push_back(place);
                }
                Declaration::Option(_) => {}
            }
        }
        (reserved, exports)
    }

    /// The function `name` of `arity` parameters of this module, as it is
    /// until its declaration is analysed.
    fn placeholder(&self, name: &crate::xml::QName, arity: usize) -> Function {
        let implementation = Implementation::Body(Body { op: Op::Empty, slots: 0 });
        Function { name: name.clone(), parameters: vec![SeqType::ANY; arity], result: SeqType::ANY, implementation, module: self.module }
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

    /// What `found` exports, analysed into the query once: of one being
    /// analysed, through a cycle of imports, the places reserved for it.
    ///
    /// # Errors
    ///
    /// As `Linked::cycle` and `Linked::open`, and the static errors of
    /// `found`.
    fn link(&mut self, found: &'e LibraryModule) -> Result<Exports> {
        if let Some(exports) = self.linked.exports(&found.name) {
            self.linked.cycle(&found.name)?;
            return Ok(exports.clone());
        }
        let depth = self.own.depth.saturating_add(1);
        let mut analysis = Analysis::library(self.env, found, std::mem::take(&mut self.linked), depth);
        analysis.begin()?;
        let exports = analysis.exported()?;
        self.linked = analysis.linked;
        self.linked.close(&found.name, exports.clone());
        Ok(exports)
    }

    /// [XQuery §4.11] Notes, for `Linked::checked_types`, the schemas the
    /// module imports beside each module it imports \[err:XQST0036\].
    pub(super) fn imported_types(&mut self) {
        let schemas: Vec<Arc<crate::xml::schema::Schema>> = self.imported.iter().map(|i| Arc::clone(&i.schema)).collect();
        for module in &self.imported_modules {
            self.linked.imported(schemas.clone(), &module.name);
        }
    }
}

#[cfg(test)]
mod tests;
