//! What the tests of the analysis and evaluation of queries share: a
//! static context of the built-in functions and collations, and a query
//! run to its value serialized, or to its error.

use super::{Collations, Environment, Input, Library, Query};
use crate::error::{Result, SqlError};
use crate::value::collation::builtin;
use crate::xml::{Item, Moment, Parameters, Sequence, Shape, Version, serialize};

/// The collations of a database (RULES.md D60).
pub(super) fn collations() -> Collations {
    builtin().into_iter().map(|c| (c.name().to_owned(), c)).collect()
}

/// The current dateTime of the tests, 2026-11-17T10:30:00Z.
pub(super) fn now() -> Moment {
    Moment::parse("2026-11-17T10:30:00Z", Shape::DateTime).unwrap().unwrap()
}

/// `text` compiled with the variables `variables` passed.
pub(super) fn compile_with(text: &str, variables: &[&str]) -> Result<Query> {
    compile_in(text, variables, &Library::builtin())
}

/// `text` compiled with the variables `variables` passed and the
/// functions of `library`.
pub(super) fn compile_in(text: &str, variables: &[&str], library: &Library) -> Result<Query> {
    let collations = collations();
    let env = Environment {
        namespaces: Vec::new(),
        default_element: None,
        variables: variables.iter().map(|v| (*v).to_owned()).collect(),
        library,
        collations: &collations,
        schemas: Vec::new(),
    };
    Query::compile(text, &env)
}

/// The value of `query` evaluated with the context item `context` and the
/// variables `variables`.
pub(super) fn evaluate(query: &Query, context: Option<Item>, variables: Vec<Sequence>) -> Result<Sequence> {
    let never = || Ok(());
    query.evaluate(Input { context_item: context, variables, now: now(), timezone: 0, interrupted: &never })
}

/// The value `value` serialized as XQuery serializes a sequence.
pub(super) fn shown(value: &Sequence) -> String {
    let parameters = Parameters { version: Version::V10, declaration: false, encoding: "UTF-8", undeclare: false };
    serialize(value, &parameters).unwrap()
}

/// The value of `text`, serialized.
pub(super) fn run(text: &str) -> String {
    let query = compile_with(text, &[]).expect(text);
    shown(&evaluate(&query, None, Vec::new()).expect(text))
}

/// The error of `text`, compiled or evaluated.
pub(super) fn error_of(text: &str) -> SqlError {
    match compile_with(text, &[]) {
        Err(e) => e,
        Ok(query) => evaluate(&query, None, Vec::new()).map(|v| shown(&v)).expect_err(text),
    }
}

/// The code of the XQuery error `text` fails with, as its message names
/// it, `err:` and the code's local name; the whole message of an error
/// that names none.
pub(super) fn code_of(text: &str) -> String {
    let message = error_of(text).msg;
    let code = message.split_whitespace().find(|w| w.starts_with("err:")).map(|w| w.trim_end_matches(':').to_owned());
    code.unwrap_or(message)
}
