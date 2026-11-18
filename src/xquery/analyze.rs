//! The static analysis of a query [XQuery §2.2.3.1]: its static context
//! made [XQuery Appendix C.1] [ISO/IEC 9075-14:2006 draft §10.20 SR2],
//! changed by its prolog [XQuery §4] (`analyze/prolog.rs`), and every name
//! of the operation tree resolved in it, each expression compiled to an
//! operation of a plan (`analyze/expr.rs`, `analyze/path.rs`,
//! `analyze/flwor.rs`, `analyze/construct.rs`, `analyze/types.rs`). A name
//! the static context does not have is a static error [err:XPST0008,
//! err:XPST0017, err:XPST0081].
//!
//! Static typing is not done: the Static Typing Feature is not supported
//! [XQuery §5.2.3], so the static types assigned are implementation-
//! dependent, here `item()*` of each expression, and every type error is
//! raised as the query is evaluated [XQuery §2.2.3.2].
//!
//! \[DESIGN\] RULES.md R-CF-6, D86: the analysis recurses as the tree
//! nests, which the parser bounds by `xquery::MAX_NESTING`.

mod construct;
mod expr;
mod flwor;
mod import;
mod path;
mod prolog;
mod scope;
mod types;

use super::ast::{EmptyOrder, Module, ModuleBody};
use super::collation::{Collations, Collator};
use super::error::{ErrorCode, error};
use super::functions::{FN_NAMESPACE, Library};
use super::plan::{Global, GlobalValue, Plan, Settings};
use std::sync::Arc;

use crate::error::Result;
use crate::xml::QName;
use crate::xml::schema::Schema;
use scope::{Namespaces, PREDECLARED, Scope};

/// \[DESIGN\] Implementation-defined [XQuery Appendix C.1]: the default
/// order for empty sequences, `empty least`.
pub const EMPTY_ORDER: EmptyOrder = EmptyOrder::Least;

/// What the SQL statement a query is in gives its static context [ISO/IEC
/// 9075-14:2006 draft §10.20 SR2d, §6.18 SR5]: the namespaces declared in
/// scope, the variables and context item passed, and the functions and
/// collations of the database.
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
}

/// [ISO/IEC 9075-14:2006 draft §10.20 SR2e] A registered XML Schema the
/// current user may use, among the in-scope schema definitions.
///
/// \[DESIGN\] Implementation-defined, RULES.md D86: of each target
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

/// The analysis of a query: its static context as it is now.
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
    /// The functions the prolog declares, each name and arity.
    declared: Vec<(QName, usize)>,
    /// The globals declared so far, each name.
    globals: Vec<QName>,
    /// Whether the query has a validate expression [XQuery §3.13].
    validates: bool,
    /// The registered XML Schemas its prolog imports [XQuery §4.10].
    imported: Vec<InScopeSchema>,
}

/// The plan of `module`, analysed in the static context `env` gives
/// [XQuery §2.2.3.1].
///
/// # Errors
///
/// SQLSTATE 10000, XQuery error, of the static error the module raises:
/// err:XQST0016 of a module import or a library module, as turborel does
/// not support the Module Feature [XQuery §5.2], or of a name, import or
/// declaration the static context refuses.
pub fn compile(module: &Module, env: &Environment<'_>) -> Result<Plan> {
    if let Some(code) = module.unsupported() {
        return Err(error(code, "turborel does not support the Module Feature [XQuery §5.2]"));
    }
    let ModuleBody::Main(prolog, body) = &module.body else {
        return Err(error(ErrorCode::XQST0016, "a library module is evaluated by no query [XQuery §4]"));
    };
    let mut analysis = Analysis::new(env);
    analysis.setup(&prolog.setup)?;
    let globals = analysis.passed();
    let (globals, functions) = analysis.declarations(&prolog.declarations, globals)?;
    let body = analysis.frame(&[], |a| a.expr(body))?.0;
    let validates = analysis.validates;
    let schemas = if validates { env.schemas.clone() } else { Vec::new() };
    let imports = analysis.imported.iter().map(|i| i.name.clone()).collect();
    Ok(Plan { body, globals, functions, settings: analysis.settings, collations: env.collations.clone(), schemas, validates, imports })
}

impl<'e> Analysis<'e> {
    /// The static context of `env` [XQuery Appendix C.1]: the predeclared
    /// namespaces and `sqlxml` [§10.20 SR2b], then those the statement
    /// declares, overriding them [§10.20 SR2d]; the default function
    /// namespace `fn`; and the rest by their defaults.
    fn new(env: &'e Environment<'e>) -> Analysis<'e> {
        let mut prefixes: Vec<(Box<str>, Box<str>)> = PREDECLARED.iter().map(|(p, u)| ((*p).into(), (*u).into())).collect();
        prefixes.extend(env.namespaces.iter().map(|(p, u)| (p.as_str().into(), u.as_str().into())));
        let default_element = env.default_element.as_deref().filter(|u| !u.is_empty()).map(Into::into);
        let names = Namespaces { prefixes, default_element, default_function: Some(FN_NAMESPACE.into()), constructed: Vec::new() };
        let settings =
            Settings { preserve: true, copy_preserve: true, copy_inherit: true, collation: Collator::codepoint(), base_uri: None };
        Analysis {
            env,
            scope: Scope::new(names),
            boundary_preserve: false,
            empty_greatest: EMPTY_ORDER == EmptyOrder::Greatest,
            settings,
            declared: Vec::new(),
            globals: Vec::new(),
            validates: false,
            imported: Vec::new(),
        }
    }

    /// The variables passed [§6.18 SR5a viii], each a global of its name
    /// in no namespace, its value the one passed at its place [§6.18
    /// GR1b].
    fn passed(&mut self) -> Vec<Global> {
        let names = self.env.variables.iter().map(|v| QName::local(v));
        let globals: Vec<Global> =
            names.enumerate().map(|(at, name)| Global { name, declared: None, value: GlobalValue::Passed(at) }).collect();
        self.globals = globals.iter().map(|g| g.name.clone()).collect();
        globals
    }
}

#[cfg(test)]
mod tests;
