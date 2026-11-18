//! The static context as a query is analysed [XQuery §2.1.1, Appendix
//! C.1]: its statically known namespaces and default namespaces, which a
//! direct element constructor's namespace declaration attributes change
//! within it [XQuery §3.7.1.2], and its in-scope variables, which an
//! expression binding one extends within the expression [XQuery §3.1.2];
//! and the resolution of a lexical QName to an expanded QName by them
//! [XQuery §2].

use std::collections::BTreeSet;

use super::super::ast::QName as Lexical;
use super::super::error::{ErrorCode, error};
use super::super::functions::FN_NAMESPACE;
use super::super::types::XDT_NAMESPACE;
use crate::error::Result;
use crate::xml::{
    Binding, QName, SQLXML_NAMESPACE, SQLXML_PREFIX, XML_NAMESPACE, XML_PREFIX, XS_NAMESPACE, XS_PREFIX, XSI_NAMESPACE, XSI_PREFIX,
};

/// [XQuery §2] The prefix `fn`.
const FN_PREFIX: &str = "fn";
/// [XQuery §2] The prefix `xdt`.
const XDT_PREFIX: &str = "xdt";
/// [XQuery §2] The prefix `local`, and its namespace.
const LOCAL_PREFIX: &str = "local";
/// The namespace `local` is bound to [XQuery §4.15].
pub(super) const LOCAL_NAMESPACE: &str = "http://www.w3.org/2005/xquery-local-functions";

/// [XQuery §2, Appendix C.1] The namespaces predeclared, and \[ISO/IEC
/// 9075-14:2006 draft §10.20 SR2b\] `sqlxml`.
pub(super) const PREDECLARED: [(&str, &str); 7] = [
    (XML_PREFIX, XML_NAMESPACE),
    (XS_PREFIX, XS_NAMESPACE),
    (XSI_PREFIX, XSI_NAMESPACE),
    (FN_PREFIX, FN_NAMESPACE),
    (XDT_PREFIX, XDT_NAMESPACE),
    (LOCAL_PREFIX, LOCAL_NAMESPACE),
    (SQLXML_PREFIX, SQLXML_NAMESPACE),
];

/// A variable in scope: a slot of the frame being compiled, or a global.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Var {
    /// A slot of the frame.
    Local(usize),
    /// A global, by its place.
    Global(usize),
}

/// The namespaces part of the static context.
#[derive(Debug, Clone, Default)]
pub(super) struct Namespaces {
    /// The statically known namespaces, each prefix to its URI, the later
    /// binding of a prefix overriding, an empty URI undeclaring it.
    pub(super) prefixes: Vec<(Box<str>, Box<str>)>,
    /// The default element/type namespace, none for "none".
    pub(super) default_element: Option<Box<str>>,
    /// The default function namespace, none for "none".
    pub(super) default_function: Option<Box<str>>,
    /// The bindings the direct element constructors being analysed make,
    /// outermost first [XQuery §3.7.4].
    pub(super) constructed: Vec<Binding>,
}

impl Namespaces {
    /// The URI `prefix` is bound to, if a statically known namespace binds
    /// it.
    pub(super) fn uri(&self, prefix: &str) -> Option<&str> {
        self.prefixes.iter().rev().find(|(p, _)| &**p == prefix).map(|(_, u)| &**u).filter(|u| !u.is_empty())
    }

    /// The prefixes bound, each once, to what binds it last.
    pub(super) fn known(&self) -> Vec<(Box<str>, Box<str>)> {
        let mut seen = BTreeSet::new();
        let mut known: Vec<(Box<str>, Box<str>)> =
            self.prefixes.iter().rev().filter(|(p, _)| seen.insert(p.clone())).filter(|(_, u)| !u.is_empty()).cloned().collect();
        known.reverse();
        known
    }
}

/// How an unprefixed name is resolved [XQuery §2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unprefixed {
    /// In the default element/type namespace: of an element or a type.
    Element,
    /// In the default function namespace.
    Function,
    /// In no namespace: of an attribute, a variable or an option.
    None,
}

/// The analysis of a query: its static context as it is now, and what the
/// frame being compiled has bound.
#[derive(Debug)]
pub(super) struct Scope {
    /// The namespaces.
    pub(super) names: Namespaces,
    /// The variables in scope, innermost last.
    variables: Vec<(QName, Var)>,
    /// How many slots the frame being compiled has so far.
    slots: usize,
    /// The globals the frame being compiled references.
    pub(super) globals_read: BTreeSet<usize>,
    /// The declared functions the frame being compiled calls.
    pub(super) functions_called: BTreeSet<usize>,
}

impl Scope {
    /// A scope of `names` and no variable.
    pub(super) fn new(names: Namespaces) -> Scope {
        Scope { names, variables: Vec::new(), slots: 0, globals_read: BTreeSet::new(), functions_called: BTreeSet::new() }
    }

    /// The expanded QName of `name` [XQuery §2]: its prefix's namespace,
    /// else as `unprefixed` says; `code` if its prefix is bound to none,
    /// err:XPST0081 but of a name test's [XQuery §3.2.1.2].
    pub(super) fn expand(&self, name: &Lexical, unprefixed: Unprefixed, code: ErrorCode) -> Result<QName> {
        let uri = match &name.prefix {
            Some(prefix) => {
                let Some(uri) = self.names.uri(prefix) else {
                    return Err(error(code, &format!("the prefix '{prefix}' of '{name}' is bound to no namespace [XQuery §2]")));
                };
                Some(uri.into())
            }
            None => match unprefixed {
                Unprefixed::Element => self.names.default_element.clone(),
                Unprefixed::Function => self.names.default_function.clone(),
                Unprefixed::None => None,
            },
        };
        Ok(QName { prefix: name.prefix.as_deref().map(Into::into), uri, local: name.local.as_str().into() })
    }

    /// As `expand`, of a name in an expression: err:XPST0081 if its prefix
    /// is bound to none.
    pub(super) fn name(&self, name: &Lexical, unprefixed: Unprefixed) -> Result<QName> {
        self.expand(name, unprefixed, ErrorCode::XPST0081)
    }

    /// A new slot of the frame, for a variable bound.
    pub(super) fn slot(&mut self) -> usize {
        let slot = self.slots;
        self.slots = self.slots.saturating_add(1);
        slot
    }

    /// `name` bound to `var` for the analysis that follows, until
    /// `unbind`.
    pub(super) fn bind(&mut self, name: QName, var: Var) {
        self.variables.push((name, var));
    }

    /// The last `n` variables bound, unbound: their scope ends.
    pub(super) fn unbind(&mut self, n: usize) {
        self.variables.truncate(self.variables.len().saturating_sub(n));
    }

    /// The variable `name` refers to: the innermost bound so [XQuery
    /// §3.1.2].
    ///
    /// # Errors
    ///
    /// err:XPST0008 if none in scope is named so.
    pub(super) fn variable(&mut self, name: &QName) -> Result<Var> {
        let Some(var) = self.variables.iter().rev().find(|(n, _)| n.same(name)).map(|(_, v)| *v) else {
            return Err(error(ErrorCode::XPST0008, &format!("no variable in scope is named ${} [XQuery §3.1.2]", display(name))));
        };
        if let Var::Global(at) = var {
            self.globals_read.insert(at);
        }
        Ok(var)
    }

    /// A new frame begun, of no slot, its globals and calls not yet noted,
    /// and its variables `variables`; the frame's slot count, globals read
    /// and functions called are what `end_frame` gives of it.
    pub(super) fn begin_frame(&mut self, variables: Vec<(QName, Var)>) -> Frame {
        Frame {
            variables: std::mem::replace(&mut self.variables, variables),
            slots: std::mem::take(&mut self.slots),
            globals_read: std::mem::take(&mut self.globals_read),
            functions_called: std::mem::take(&mut self.functions_called),
        }
    }

    /// The frame begun ended, the one before it back; what it bound and
    /// referenced.
    pub(super) fn end_frame(&mut self, saved: Frame) -> Frame {
        Frame {
            variables: std::mem::replace(&mut self.variables, saved.variables),
            slots: std::mem::replace(&mut self.slots, saved.slots),
            globals_read: std::mem::replace(&mut self.globals_read, saved.globals_read),
            functions_called: std::mem::replace(&mut self.functions_called, saved.functions_called),
        }
    }
}

/// A frame's analysis: its variables, slot count, and the globals and
/// declared functions it references.
#[derive(Debug, Default)]
pub(super) struct Frame {
    /// The variables in scope.
    variables: Vec<(QName, Var)>,
    /// How many slots.
    pub(super) slots: usize,
    /// The globals read.
    pub(super) globals_read: BTreeSet<usize>,
    /// The declared functions called.
    pub(super) functions_called: BTreeSet<usize>,
}

/// `name` written as `{uri}local`, or `local` of no namespace.
pub(super) fn display(name: &QName) -> String {
    match &name.uri {
        Some(uri) => format!("{{{uri}}}{}", name.local),
        None => name.local.to_string(),
    }
}
