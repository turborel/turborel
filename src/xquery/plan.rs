//! The plan a query is compiled to by its static analysis [XQuery
//! §2.2.3.1], which its evaluation runs [XQuery §2.2.3.2]: the operation
//! tree of the query's expressions with every name resolved in the
//! static context, each variable a slot of a frame, each function call
//! its function, each name test and constructor's name an expanded QName
//! (`plan/op.rs`), and the prolog's declarations, with those of the
//! library modules it imports [XQuery §4.11].
//!
//! \[DESIGN\] RULES.md D106: a plan is made once, as the statement is bound,
//! shared by every evaluation of it and read by none but them; nothing in
//! it changes as a query is evaluated. The functions and variables of
//! every module of the query, the main module's and those of each library
//! module imported at any remove, are one list of each, so a call or a
//! reference is a place in it whichever module declares what it names;
//! each declaration keeps the module it is of, whose static context its
//! evaluation reads (`ModuleContext`).

mod op;

use std::sync::Arc;

pub use op::{
    Arith, Axis, CallOp, Callee, CastTarget, Clause, CompareOp, ComputedElement, Content, DirectElement, FlworOp, NameOp, Namespace, Op,
    OrderKey, Part, PathOp, PathStart, PiOp, QuantifiedOp, StaticNames, StepOp, StepTest, TypeswitchOp,
};

use super::analyze::InScopeSchema;
use super::collation::{Collations, Collator};
use super::functions::XQueryFunction;
use super::types::SeqType;
use crate::xml::QName;

/// A query, analysed and compiled [XQuery §2.2.3.1].
pub struct Plan {
    /// The query body.
    pub(super) body: Body,
    /// The variables of every module's prolog, and those passed in
    /// [XQuery §4.14].
    pub(super) globals: Vec<Global>,
    /// The functions every module's prolog declares [XQuery §4.15].
    pub(super) functions: Vec<Function>,
    /// What the evaluation reads of the main module's static context.
    pub(super) main: ModuleContext,
    /// What it reads of each library module's, the `n`th module's at
    /// `n - 1`, the main module being the 0th.
    pub(super) libraries: Vec<ModuleContext>,
    /// The statically known collations: the database's [XQuery §2.1.1].
    pub(super) collations: Collations,
    /// Whether the main module has a validate expression [XQuery §3.13].
    pub(super) validates: bool,
    /// The names of the registered XML Schemas the main module's prolog
    /// imports [XQuery §4.10].
    pub(super) imports: Vec<String>,
    /// The names of the registered XQuery modules the main module's prolog
    /// imports [XQuery §4.11].
    pub(super) imported_modules: Vec<String>,
}

/// What the evaluation reads of one module's static context [XQuery
/// §2.1.1]: each module has its own [XQuery §4.11].
#[derive(Debug, Clone)]
pub struct ModuleContext {
    /// Its settings.
    pub(super) settings: Settings,
    /// The in-scope schema definitions its validate expressions read
    /// [ISO/IEC 9075-14:2006 draft §10.20 SR2e]; none of a module that has
    /// no validate expression.
    pub(super) schemas: Vec<InScopeSchema>,
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("body", &self.body)
            .field("globals", &self.globals)
            .field("functions", &self.functions)
            .field("main", &self.main)
            .field("libraries", &self.libraries)
            .finish_non_exhaustive()
    }
}

impl Plan {
    /// What the evaluation reads of the `module`th module's static
    /// context: the main module's of 0, and of a place no library module
    /// has, which no plan's declaration names.
    pub(super) fn context(&self, module: usize) -> &ModuleContext {
        module.checked_sub(1).and_then(|at| self.libraries.get(at)).unwrap_or(&self.main)
    }
}

/// An expression and the slots of the frame it binds its variables in.
#[derive(Debug)]
pub struct Body {
    /// The expression.
    pub(super) op: Op,
    /// How many slots its frame has.
    pub(super) slots: usize,
}

/// A variable of the prolog, or one passed in [XQuery §4.14] [ISO/IEC
/// 9075-14:2006 draft §6.18 SR5a viii].
#[derive(Debug)]
pub struct Global {
    /// Its name.
    pub(super) name: QName,
    /// Its declared type, if it has one.
    pub(super) declared: Option<SeqType>,
    /// Where its value comes from.
    pub(super) value: GlobalValue,
    /// The module whose prolog declares it, as `Plan::context` numbers
    /// them.
    pub(super) module: usize,
}

/// Where a variable of the prolog takes its value from.
#[derive(Debug)]
pub enum GlobalValue {
    /// Its initializing expression.
    Initialized(Body),
    /// The value passed in at this place of those passed [ISO/IEC
    /// 9075-14:2006 draft §6.18 GR1b].
    Passed(usize),
    /// None: it is declared `external`, and no value is passed for it
    /// [XQuery §4.14].
    Missing,
}

/// A function the prolog declares [XQuery §4.15].
#[derive(Debug)]
pub struct Function {
    /// Its name.
    pub(super) name: QName,
    /// Its parameters' declared types, `item()*` of none.
    pub(super) parameters: Vec<SeqType>,
    /// Its declared result type, `item()*` of none.
    pub(super) result: SeqType,
    /// What computes it.
    pub(super) implementation: Implementation,
    /// The module whose prolog declares it, as `Plan::context` numbers
    /// them.
    pub(super) module: usize,
}

/// What computes a declared function.
pub enum Implementation {
    /// Its body, its parameters the first slots of its frame.
    Body(Body),
    /// \[EXTENSION\] An external function: the library's of its name and
    /// arity [XQuery §4.15].
    External(Arc<dyn XQueryFunction>),
}

impl std::fmt::Debug for Implementation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Implementation::Body(body) => write!(f, "Body({body:?})"),
            Implementation::External(function) => write!(f, "External({:?})", function.name()),
        }
    }
}

/// The components of the static context the evaluation reads [XQuery
/// §2.1.1, Appendix C.1].
#[derive(Debug, Clone)]
pub struct Settings {
    /// Construction mode: whether it is `preserve` [XQuery §4.6].
    pub(super) preserve: bool,
    /// Copy-namespaces mode: whether it preserves [XQuery §4.9].
    pub(super) copy_preserve: bool,
    /// Copy-namespaces mode: whether it inherits.
    pub(super) copy_inherit: bool,
    /// The default collation [XQuery §4.4].
    pub(super) collation: Collator,
    /// The base URI, if declared [XQuery §4.5].
    pub(super) base_uri: Option<Box<str>>,
}

impl Settings {
    /// The settings of a static context no prolog has changed [XQuery
    /// Appendix C.1]: construction mode preserve, copy-namespaces mode
    /// preserve and inherit, the codepoint collation, no base URI.
    pub(super) fn initial() -> Settings {
        Settings { preserve: true, copy_preserve: true, copy_inherit: true, collation: Collator::codepoint(), base_uri: None }
    }
}
