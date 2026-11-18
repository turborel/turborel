//! The built-in functions [XQueryFO]: each function of the library of
//! XQuery 1.0 Functions and Operators (`xquery::library`), a row of its
//! section's table, made an `XQueryFunction` of each number of arguments
//! it takes, or one that takes any number past its least.
//!
//! \[DESIGN\] RULES.md D86: a built-in function declares each parameter
//! and its result `item()*`, as it applies the function conversion rules
//! of its own signature to its arguments itself [XQuery §3.1.5], so the
//! evaluation converts nothing before it.

use std::sync::Arc;

use super::super::library::{self, Body};
use super::super::types::SeqType;
use super::{Context, FN_NAMESPACE, XQueryFunction};
use crate::error::Result;
use crate::xml::{QName, Sequence};

/// A built-in function of one arity, or of any number of arguments from
/// its parameters' on.
struct Native {
    /// Its name, in the namespace `fn`.
    name: QName,
    /// Its parameters, each `item()*`.
    parameters: Vec<SeqType>,
    /// Whether it takes any number of arguments past its parameters'.
    variadic: bool,
    /// What it computes.
    body: Body,
}

impl XQueryFunction for Native {
    fn name(&self) -> &QName {
        &self.name
    }

    fn parameters(&self) -> &[SeqType] {
        &self.parameters
    }

    fn result(&self) -> &SeqType {
        &SeqType::ANY
    }

    fn variadic(&self) -> bool {
        self.variadic
    }

    fn call(&self, context: &Context<'_>, arguments: Vec<Sequence>) -> Result<Sequence> {
        (self.body)(context, &arguments)
    }
}

/// The function of the name `local` and `arity` parameters, taking more
/// if `variadic`, `body` its value.
fn native(local: &str, arity: usize, variadic: bool, body: Body) -> Arc<dyn XQueryFunction> {
    let name = QName { prefix: None, uri: Some(FN_NAMESPACE.into()), local: local.into() };
    Arc::new(Native { name, parameters: vec![SeqType::ANY; arity], variadic, body })
}

/// Every built-in function: of each row of the library, one of each arity
/// it takes, or one variadic of its least if it takes any number.
pub(super) fn all() -> Vec<Arc<dyn XQueryFunction>> {
    let mut out = Vec::new();
    for f in library::all() {
        if f.most == usize::MAX {
            out.push(native(f.local, f.least, true, f.body));
        } else {
            out.extend((f.least..=f.most).map(|arity| native(f.local, arity, false, f.body)));
        }
    }
    out
}
