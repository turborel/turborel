//! XQuery's Static Typing Feature [XQuery §5.2.3]: the static semantics of
//! XQuery 1.0 and XPath 2.0 Formal Semantics, the Candidate
//! Recommendation of 3 November 2005, \[XQueryFS\], applied to a compiled
//! query (`xquery/plan.rs`), every type error detected as it is analysed
//! [XQuery §2.2.3.1] and so as the SQL statement it is in is prepared
//! [ISO/IEC 9075-14:2006 draft §6.18 SR6].
//!
//! Each expression of the plan is given its static type by the judgment
//! `statEnv |- Expr : Type` of its normalization into the Core [XQueryFS
//! §3.2.3, §4]: the plan's operations are the expressions the analysis
//! resolved, so each is typed as its Core normal form is, its implicit
//! operations, atomization, the effective boolean value, the conversion of
//! untyped operands and function arguments, typed as [XQueryFS §7] types
//! them. An operand of a type not appropriate to it is err:XPTY0004
//! [XQuery §2.2.3.1], or the more particular type error XQuery names; an
//! expression of the empty type but `()` and `fn:data(())` is
//! err:XPST0005 [XQuery §2.3.1]; the context item read where its static
//! type is none is err:XPST0001 [XQuery Appendix C.1].
//!
//! The types are \[XQueryFS\]'s (`statics/types.rs`), sequence types
//! normalized to them (`statics/sequence.rs`), compared by subtyping and
//! promotion (`statics/subtype.rs`); the types of the in-scope schema
//! definitions expand to their content (`statics/expand.rs`) for the axes
//! (`statics/axes.rs`); the operators (`statics/operators.rs`), the
//! implicit operations (`statics/values.rs`), the functions
//! (`statics/functions.rs`), the constructors (`statics/constructors.rs`)
//! and the expressions (`statics/expr.rs`) each by their rules. A
//! variable or context item SQL/XML passes is of the formal type §10.21
//! determines (`Formal`).
//!
//! \[DESIGN\] RULES.md D106, R-CF-6: the typing recurses as the plan nests,
//! counting each expression typed within another, and is SQLCODE -900
//! past `xquery::MAX_DEPTH`; each walk of a type counts its depth too,
//! bounded by `MAX_TYPE_DEPTH`.

mod axes;
mod constructors;
mod expand;
mod expr;
mod functions;
mod names;
mod operators;
mod sequence;
mod signatures;
mod subtype;
mod types;
mod values;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use super::analyze::InScopeSchema;
use super::ast::Occurrence;
use super::error::{ErrorCode, error};
use super::plan::{GlobalValue, Implementation, Plan};
use super::types::{SchemaType, SeqType};
use crate::error::{Result, SqlError};
use crate::xml::AtomicType;
use crate::xml::schema::Schema;
use sequence::{any_attribute, any_element, normalized};
pub(super) use types::Type;
use types::{Atom, Item, Name, Node, Occ};

pub use types::MAX_TYPE_DEPTH;

/// [ISO/IEC 9075-14:2006 draft §10.21] The formal type notation of a
/// value SQL/XML passes a query, of its declared type.
#[derive(Debug, Clone)]
pub struct Formal {
    /// What its type is.
    pub kind: FormalKind,
    /// Its suffix [§10.21 SR3]: none, `*` or `?`.
    pub occurrence: Occurrence,
}

/// [ISO/IEC 9075-14:2006 draft §10.21 SR5] The kinds of formal type: of
/// each XML type, and of an SQL predefined type its XML Schema primitive
/// type.
#[derive(Debug, Clone)]
pub enum FormalKind {
    /// XML(DOCUMENT(..)) [SR5a to SR5c].
    Document(Typed),
    /// XML(CONTENT(..)) [SR5d to SR5f].
    Content(Typed),
    /// XML(SEQUENCE) \[SR5g\].
    Sequence,
    /// An SQL predefined type: the atomic type \[SR5h\].
    Atomic(AtomicType),
}

/// The secondary type modifier of an XML type [§10.21 SR4, SR5].
#[derive(Debug, Clone)]
pub enum Typed {
    /// UNTYPED: `xdt:untyped`.
    Untyped,
    /// ANY: `xs:anyType`.
    Any,
    /// XMLSCHEMA: the global element declarations of the registered XML
    /// Schema's schema, of the namespace and element its descriptor
    /// includes, if any \[SR4b\].
    Schema(Arc<Schema>, Option<String>, Option<String>),
}

/// What SQL/XML passes a query, typed [§6.18 SR5]: its context item's
/// formal type, if one is passed, and each variable's, in their order.
#[derive(Debug, Clone, Default)]
pub struct Passed {
    /// The context item's.
    pub context: Option<Formal>,
    /// The variables'.
    pub variables: Vec<Formal>,
    /// The context item's type, of an item of the value of a query typed
    /// before it (`Query::row_item`), which goes before `context`'s.
    pub(super) item: Option<Type>,
}

/// The error of a type error, `why` naming the rule [XQuery §2.2.3.1]:
/// err:XPTY0004.
pub(super) fn type_error(why: &str) -> SqlError {
    error(ErrorCode::XPTY0004, why)
}

/// The error of an evaluation past turborel's limits, `why` (RULES.md D18).
pub(super) fn beyond(why: &str) -> SqlError {
    super::error::beyond(why)
}

/// [XQuery Appendix C.1] The error of reading the context item, by
/// `what`, where its static type is none: of no context item passed, or
/// within a function body [XQuery §4.15] \[err:XPST0001\].
pub(super) fn no_focus(what: &str) -> SqlError {
    error(ErrorCode::XPST0001, &format!("{what} reads the context item, whose static type is none [XQuery Appendix C.1] [XQueryFS §4.1.4]"))
}

/// [XQuery §2.3.1] The error of an expression of the empty type, `why`
/// \[err:XPST0005\].
pub(super) fn empty_type(why: &str) -> SqlError {
    error(ErrorCode::XPST0005, why)
}

/// What the typing knows of a query: its plan, the types of its globals
/// and functions, the context item's, and the in-scope schema definitions
/// a validate expression validates by.
pub(super) struct Statics<'a> {
    /// The plan.
    plan: &'a Plan,
    /// Each global's type, as far as typed.
    globals: Vec<Type>,
    /// Each declared function's parameters' and result's types.
    functions: Vec<(Vec<Type>, Type)>,
    /// The context item's type, none if no context item is passed.
    context: Option<Type>,
    /// The in-scope schema definitions [§10.20 SR2e].
    schemas: &'a [InScopeSchema],
    /// How many expressions deep the typing is.
    depth: usize,
    /// The module whose expression is typed, as `Plan::context` numbers
    /// them [XQuery §4.11]: each reads its own static context.
    module: usize,
}

/// The typing of one body: each slot's type, as bound, and the focus's,
/// the context item's type, if one is defined.
pub(super) struct Frame {
    /// The slots' types.
    slots: Vec<Option<Type>>,
    /// The focus.
    focus: Option<Type>,
}

/// [XQuery §5.2.3] `plan` typed statically, of the values `passed` and the
/// in-scope schema definitions `schemas`: the prime type of its body's,
/// the type of an item of its value [XQueryFS §8.4].
///
/// # Errors
///
/// SQLSTATE 10000, XQuery error, of the first type error or static error
/// the typing finds [ISO/IEC 9075-14:2006 draft §6.18 SR6, GR3]; SQLCODE
/// -900 past turborel's limits.
pub fn check(plan: &Plan, passed: &Passed, schemas: &[InScopeSchema]) -> Result<Type> {
    let functions = plan.functions.iter().map(|f| (f.parameters.iter().map(normalized).collect(), normalized(&f.result))).collect();
    let context = passed.item.clone().or_else(|| passed.context.as_ref().map(|f| formal(f, true)));
    let mut s = Statics { plan, globals: Vec::new(), functions, context, schemas, depth: 0, module: 0 };
    for global in &plan.globals {
        let t = s.global(global, passed)?;
        s.globals.push(t);
    }
    for (at, f) in plan.functions.iter().enumerate() {
        if let Implementation::Body(body) = &f.implementation {
            s.function_body(at, body)?;
        }
    }
    s.module = 0;
    let mut frame = Frame { slots: vec![None; plan.body.slots], focus: s.context.clone() };
    let body = s.expr(&plan.body.op, &mut frame)?;
    Ok(Type::of_prime(types::prime(&body)?, Occ::One))
}

impl Statics<'_> {
    /// [XQueryFS §5.14] A global's type: of one passed, its formal type; of
    /// one declared, its declared type, checked against its initializing
    /// expression's; else that expression's, or `item()*` of an external
    /// one. A library module's initializing expression has no focus
    /// [XQuery §2.1.2, §4.14].
    fn global(&mut self, global: &super::plan::Global, passed: &Passed) -> Result<Type> {
        let declared = global.declared.as_ref().map(normalized);
        self.module = global.module;
        let typed = match &global.value {
            GlobalValue::Passed(at) => passed.variables.get(*at).map(|f| formal(f, false)),
            GlobalValue::Initialized(body) => {
                let focus = if global.module == 0 { self.context.clone() } else { None };
                let mut frame = Frame { slots: vec![None; body.slots], focus };
                Some(self.expr(&body.op, &mut frame)?)
            }
            GlobalValue::Missing => None,
        };
        match (typed, declared) {
            (Some(t), Some(d)) => {
                if !subtype::subtype(&t, &d)? {
                    let why =
                        format!("the variable ${} is declared {d} and initialized with a value of {t} [XQueryFS §5.14]", global.name.local);
                    return Err(type_error(&why));
                }
                Ok(d)
            }
            (Some(t), None) => Ok(t),
            (None, Some(d)) => Ok(d),
            (None, None) => Ok(normalized(&SeqType::ANY)),
        }
    }

    /// [XQueryFS §5.15] The body of declared function `at` typed with its
    /// parameters of their declared types, no focus defined [XQuery
    /// §4.15]: of a type that is a subtype of its declared result type.
    fn function_body(&mut self, at: usize, body: &super::plan::Body) -> Result<()> {
        let Some((parameters, result)) = self.functions.get(at).cloned() else { return Ok(()) };
        self.module = self.plan.functions.get(at).map_or(0, |f| f.module);
        let mut slots = vec![None; body.slots];
        for (slot, p) in slots.iter_mut().zip(parameters) {
            *slot = Some(p);
        }
        let mut frame = Frame { slots, focus: None };
        let t = self.expr(&body.op, &mut frame)?;
        if !subtype::subtype(&t, &result)? {
            let name = self.plan.functions.get(at).map(|f| f.name.local.to_string()).unwrap_or_default();
            let why = format!("the body of the function {name} is of {t}, not of its declared {result} [XQueryFS §5.15]");
            return Err(type_error(&why));
        }
        Ok(())
    }
}

/// [ISO/IEC 9075-14:2006 draft §10.21 SR5] The type `f` notes, of the
/// context item if `context`, its suffix none [SR3a].
#[must_use]
pub fn formal(f: &Formal, context: bool) -> Type {
    let occ = if context { Occ::One } else { Occ::from(f.occurrence) };
    let others = || Type::of_prime(vec![Item::Comment, Item::Pi], Occ::Star);
    let t = match &f.kind {
        FormalKind::Document(typed) => {
            let children = Type::Seq(vec![others(), elements(typed), others()]);
            Type::Item(Item::Document(Box::new(children)))
        }
        FormalKind::Content(typed) => {
            let mut items = sequence_items(&elements(typed));
            items.extend([Item::Comment, Item::Pi]);
            if !matches!(typed, Typed::Schema(..)) {
                items.push(Item::Text);
            }
            Type::Item(Item::Document(Box::new(Type::of_prime(items, Occ::Star))))
        }
        FormalKind::Sequence => {
            let mut items = vec![any_element(), any_attribute(), Item::Text, Item::Comment, Item::Pi, Item::Atomic(Atom::Any)];
            items.push(Item::Document(Box::new(Type::of_prime(vec![any_element(), Item::Comment, Item::Pi, Item::Text], Occ::Star))));
            Type::of_prime(items, Occ::One)
        }
        FormalKind::Atomic(a) => Type::atomic(*a),
    };
    Type::occur(t, occ)
}

/// The element types of `typed` [§10.21 SR4, SR5]: `element * of type
/// xdt:untyped` or `xs:anyType`, or the global element declarations of
/// the registered XML Schema indicated.
fn elements(typed: &Typed) -> Type {
    let of = match typed {
        Typed::Untyped => SchemaType::Untyped,
        Typed::Any => SchemaType::AnyType,
        Typed::Schema(schema, namespace, element) => {
            let name = namespace.as_ref().map(|ns| match element {
                Some(e) => Name::new(Some(ns).filter(|n| !n.is_empty()).map(String::as_str), e),
                None => Name {
                    namespace: super::plan::Namespace::of((!ns.is_empty()).then(|| ns.as_str().into())),
                    local: None,
                    narrowed: None,
                },
            });
            return expand::declared_elements(schema, name.as_ref());
        }
    };
    Type::Item(Item::Element(Box::new(Node { name: Name::ANY, nillable: false, of })))
}

/// The item types of `t`, a choice of them.
fn sequence_items(t: &Type) -> Vec<Item> {
    types::prime(t).unwrap_or_default()
}
