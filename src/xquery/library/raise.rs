//! The error function [XQueryFO §3], which raises an error a query names,
//! and the trace function [XQueryFO §4].
//!
//! \[DESIGN\] RULES.md D106: an error a query raises is SQL/XML's XQuery
//! error [ISO/IEC 9075-14:2006 draft §6.18 GR3], its message the error's
//! QName as the URI `NS#LP` [XQueryFO §3] and its description; an error
//! object is not returned, as [XQueryFO §3] leaves to the implementation.
//! The trace data set, whose destination is implementation-defined
//! [XQueryFO §4], is discarded: no statement's evaluation writes where
//! another session or the server's log would read it.

use super::super::error::{ErrorCode, error, named_error};
use super::super::functions::Context;
use super::args::{arg, optional_atomic, string, wrong_type};
use super::{Builtin, ERR_NAMESPACE};
use crate::error::{Result, SqlError};
use crate::xml::{Atomic, Primitive, Sequence};

/// The functions of [XQueryFO §3, §4].
pub(super) const FUNCTIONS: [Builtin; 2] = [Builtin::function("error", 0, 3, raise), Builtin::function("trace", 2, 2, trace)];

/// The error the QName `name` names, described by `description`: of the
/// namespace of XQuery's errors, as the library raises its own; else as
/// the URI `NS#LP` [XQueryFO §3].
fn raised(name: &Atomic, description: &str) -> SqlError {
    let local = name.lexical.split_once(':').map_or(&*name.lexical, |(_, l)| l);
    match name.namespace.as_deref() {
        Some(ERR_NAMESPACE) => named_error(&format!("err:{local}"), description),
        uri => named_error(&format!("{}#{local}", uri.unwrap_or_default()), description),
    }
}

/// [XQueryFO §3] `fn:error($error as xs:QName?, $description as
/// xs:string, $error-object as item()*) as none`: the error its QName
/// names, err:FOER0000 if none is given or it is the empty sequence.
///
/// # Errors
///
/// Always: the error named; err:XPTY0004 of an argument of the wrong
/// type.
fn raise(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let description = match args.get(1) {
        Some(given) => string(given)?,
        None => String::new(),
    };
    let Some(name) = optional_atomic(arg(args, 0))? else {
        if args.len() == 1 {
            return Err(wrong_type("an xs:QName", "it is the empty sequence"));
        }
        return Err(error(ErrorCode::FOER0000, if description.is_empty() { "fn:error was called [XQueryFO §3]" } else { &description }));
    };
    if name.kind.primitive() != Primitive::QName {
        return Err(wrong_type("an xs:QName", &format!("it is an {}", name.kind.name())));
    }
    Err(raised(&name, &description))
}

/// [XQueryFO §4] `fn:trace($value as item()*, $label as xs:string) as
/// item()*`: `$value`, unchanged, its trace discarded.
///
/// # Errors
///
/// err:XPTY0004 of a label that is not one string.
fn trace(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    string(arg(args, 1))?;
    Ok(arg(args, 0).clone())
}

#[cfg(test)]
mod tests;
