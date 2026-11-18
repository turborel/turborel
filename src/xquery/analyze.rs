//! The static analysis of a query [XQuery §2.2.3.1]: its static context
//! made [XQuery Appendix C.1] [ISO/IEC 9075-14:2006 draft §10.20 SR2],
//! changed by its prolog [XQuery §4] (`analyze/prolog.rs`), and every name
//! of the operation tree resolved in it, each expression compiled to an
//! operation of a plan (`analyze/expr.rs`, `analyze/path.rs`,
//! `analyze/flwor.rs`, `analyze/construct.rs`, `analyze/types.rs`). A name
//! the static context does not have is a static error [err:XPST0008,
//! err:XPST0017, err:XPST0081]. The library modules a module imports are
//! analysed, each in a static context of its own, into the one plan
//! (`analyze/modules.rs`, `analyze/linked.rs`) [XQuery §4.11].
//!
//! Static typing is not done: the Static Typing Feature is not supported
//! [XQuery §5.2.3], so the static types assigned are implementation-
//! dependent, here `item()*` of each expression, and every type error is
//! raised as the query is evaluated [XQuery §2.2.3.2].
//!
//! \[DESIGN\] RULES.md R-CF-6, D106: the analysis recurses as the tree
//! nests, which the parser bounds by `xquery::MAX_NESTING`, and as modules
//! import modules, which `MAX_IMPORT_DEPTH` bounds.

mod construct;
mod expr;
mod flwor;
mod import;
mod linked;
mod modules;
mod path;
mod prolog;
mod scope;
mod types;

use super::ast::{EmptyOrder, Module, ModuleBody};
use super::collation::Collations;
use super::error::{ErrorCode, error};
use super::functions::{FN_NAMESPACE, Library};
use super::plan::{Global, GlobalValue, ModuleContext, Plan, Settings};
use std::sync::Arc;

use crate::error::Result;
use crate::xml::QName;
use crate::xml::schema::Schema;
pub use linked::MAX_IMPORT_DEPTH;
use linked::{Linked, Signature};
pub use modules::{Analysed, LibraryModule, library_module};
use scope::{Namespaces, PREDECLARED, Scope};

/// \[DESIGN\] Implementation-defined [XQuery Appendix C.1]: the default
/// order for empty sequences, `empty least`.
pub const EMPTY_ORDER: EmptyOrder = EmptyOrder::Least;

/// What the SQL statement a query is in gives its static context [ISO/IEC
/// 9075-14:2006 draft §10.20 SR2d, §6.18 SR5]: the namespaces declared in
/// scope, the variables and context item passed, and the functions,
/// collations, registered XML Schemas and registered XQuery modules of the
/// database.
pub struct Environment<'a> {
    /// The prefixes `<XML namespace declaration>`s in scope bind, each to
    /// its URI, the innermost last [§10.20 SR2d i].
    pub namespaces: Vec<(String, String)>,
    /// The default namespace the innermost declaration of one in scope
    /// sets, the zero-length string for NO DEFAULT; none if none does
    /// [§10.20 SR2d ii].
    pub default_element: Option<String>,
    /// The names of the variables passed, each an XML 1.1 NCName, in the
    /// order their values are given [§6.18 SR5a].
    pub variables: Vec<String>,
    /// The functions of the database [XQuery §2.1.1].
    pub library: &'a Library,
    /// The collations of the database (RULES.md D60).
    pub collations: &'a Collations,
    /// The in-scope schema definitions [§10.20 SR2e], as validate takes
    /// them (`InScopeSchema`).
    pub schemas: Vec<InScopeSchema>,
    /// The registered XML Schemas a schema import may import [XQuery
    /// §4.10]: those the current user may use, in the order of their
    /// names, the permanently registered ones last.
    pub importable: Vec<InScopeSchema>,
    /// The registered XQuery modules a module import of the query may
    /// import [XQuery §4.11]: those the current user may use, in the order
    /// of their names.
    pub modules: Vec<LibraryModule>,
    /// The registered XQuery modules those import, at any remove, each as
    /// it was registered to (`LibraryModule::modules`), whether the
    /// current user may use it or not.
    pub linked: Vec<LibraryModule>,
}

/// [ISO/IEC 9075-14:2006 draft §10.20 SR2e] A registered XML Schema the
/// current user may use, among the in-scope schema definitions.
///
/// \[DESIGN\] Implementation-defined, RULES.md D106: of each target
/// namespace, the one XMLVALIDATE's data-driven validation chooses, of the
/// least name, the permanently registered one of a namespace no other has,
/// chosen as the statement is bound. A validate expression takes, as its
/// effective schema [XQuery §3.13], the one of the namespace of the
/// element it validates, or none.
#[derive(Debug, Clone)]
pub struct InScopeSchema {
    /// Its target namespace URI, the zero-length string of none.
    pub namespace: String,
    /// Its `<registered XML Schema name>`.
    pub name: String,
    /// Its schema location URI.
    pub location: String,
    /// Its schema, read.
    pub schema: Arc<Schema>,
}

/// What a module's static context is made of beside what every module's
/// is [XQuery §4.11]: of the main module, what the statement gives; of a
/// library module, what it was registered with.
struct Own<'e> {
    /// The registered XQuery module, of a library module.
    library: Option<&'e LibraryModule>,
    /// The registered XML Schemas its schema imports may import [XQuery
    /// §4.10].
    importable: &'e [InScopeSchema],
    /// Its in-scope schema definitions [§10.20 SR2e].
    schemas: &'e [InScopeSchema],
    /// The registered XQuery modules its module imports may import
    /// [XQuery §4.11].
    modules: Vec<&'e LibraryModule>,
    /// How many module imports lead to it from the main module.
    depth: usize,
}

/// The analysis of a module of a query: its static context as it is now.
struct Analysis<'e> {
    /// What the statement gives.
    env: &'e Environment<'e>,
    /// The namespaces and variables in scope.
    scope: Scope,
    /// Whether the boundary-space policy is `preserve` [XQuery §4.3].
    boundary_preserve: bool,
    /// Whether the default order for empty sequences is `greatest`
    /// [XQuery §4.8].
    empty_greatest: bool,
    /// The settings the evaluation reads.
    settings: Settings,
    /// Its function signatures: those its prolog declares and those of the
    /// modules it imports [XQuery §4.15].
    declared: Vec<Signature>,
    /// Its in-scope variables of the prolog so far, each name and place:
    /// those passed in, those of the modules it imports, and those its
    /// prolog has declared [XQuery §4.14].
    globals: Vec<(QName, usize)>,
    /// Whether the module has a validate expression [XQuery §3.13].
    validates: bool,
    /// The registered XML Schemas its prolog imports [XQuery §4.10].
    imported: Vec<InScopeSchema>,
    /// What its static context is made of.
    own: Own<'e>,
    /// The registered XQuery modules its prolog imports [XQuery §4.11].
    imported_modules: Vec<&'e LibraryModule>,
    /// What every module of the query declares, as it is analysed.
    linked: Linked,
    /// Its place among the modules of the query, as `Plan::context`
    /// numbers them.
    module: usize,
}

/// The plan of `module`, a main module, analysed in the static context
/// `env` gives [XQuery §2.2.3.1], with the library modules it imports.
///
/// # Errors
///
/// SQLSTATE 10000, XQuery error, of the static error the module raises:
/// err:XPST0003 of a library module, which no query evaluates [XQuery §4];
/// or of a name, import or declaration the static context refuses.
pub fn compile(module: &Module, env: &Environment<'_>) -> Result<Plan> {
    let ModuleBody::Main(prolog, body) = &module.body else {
        return Err(error(ErrorCode::XPST0003, "a query is a main module: no query evaluates a library module [XQuery §4]"));
    };
    let own = Own { library: None, importable: &env.importable, schemas: &env.schemas, modules: env.modules.iter().collect(), depth: 0 };
    let mut analysis = Analysis::new(env, own, Linked::default());
    analysis.setup(&prolog.setup)?;
    analysis.passed();
    analysis.declarations(&prolog.declarations)?;
    let body = analysis.frame(&[], |a| a.expr(body))?.0;
    analysis.linked.circularity()?;
    let main = analysis.context();
    let imports = analysis.imported.iter().map(|i| i.name.clone()).collect();
    let imported_modules = analysis.imported_modules.iter().map(|m| m.name.clone()).collect();
    let validates = analysis.validates;
    let (globals, functions, libraries) = analysis.linked.into_parts();
    Ok(Plan { body, globals, functions, main, libraries, collations: env.collations.clone(), validates, imports, imported_modules })
}

impl<'e> Analysis<'e> {
    /// The static context of a module of `env` made of `own` [XQuery
    /// Appendix C.1], its declarations to be added to `linked`: the
    /// predeclared namespaces and `sqlxml` [§10.20 SR2b], then, of the
    /// main module, those the statement declares, overriding them, and its
    /// default element namespace [§10.20 SR2d]; the default function
    /// namespace `fn`; and the rest by their defaults. A library module's
    /// static context is its own, of nothing the statement declares
    /// [XQuery §4.11].
    fn new(env: &'e Environment<'e>, own: Own<'e>, mut linked: Linked) -> Analysis<'e> {
        let main = own.library.is_none();
        let mut prefixes: Vec<(Box<str>, Box<str>)> = PREDECLARED.iter().map(|(p, u)| ((*p).into(), (*u).into())).collect();
        prefixes.extend(env.namespaces.iter().filter(|_| main).map(|(p, u)| (p.as_str().into(), u.as_str().into())));
        let default_element = env.default_element.as_deref().filter(|u| main && !u.is_empty()).map(Into::into);
        let names = Namespaces { prefixes, default_element, default_function: Some(FN_NAMESPACE.into()), constructed: Vec::new() };
        let module = linked.begin();
        Analysis {
            env,
            scope: Scope::new(names),
            boundary_preserve: false,
            empty_greatest: EMPTY_ORDER == EmptyOrder::Greatest,
            settings: Settings::initial(),
            declared: Vec::new(),
            globals: Vec::new(),
            validates: false,
            imported: Vec::new(),
            own,
            imported_modules: Vec::new(),
            linked,
            module,
        }
    }

    /// The variables passed [§6.18 SR5a viii], each a global of its name
    /// in no namespace, its value the one passed at its place [§6.18
    /// GR1b], in scope of the main module.
    fn passed(&mut self) {
        for (at, name) in self.env.variables.iter().map(|v| QName::local(v)).enumerate() {
            let global = Global { name: name.clone(), declared: None, value: GlobalValue::Passed(at), module: self.module };
            let place = self.linked.global(global, None);
            self.globals.push((name, place));
        }
    }

    /// What the evaluation reads of its static context, once analysed.
    fn context(&self) -> ModuleContext {
        let schemas = if self.validates { self.own.schemas.to_vec() } else { Vec::new() };
        ModuleContext { settings: self.settings.clone(), schemas }
    }
}

#[cfg(test)]
mod tests;
