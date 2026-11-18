//! What the tests of the analysis and evaluation of queries share: a
//! static context of the built-in functions and collations, and a query
//! run to its value serialized, or to its error.

use std::sync::Arc;

use super::{Collations, Environment, Input, Library, Query};
use crate::error::{Result, SqlError};
use crate::value::collation::{Collation, Pad, Unit, builtin};
use crate::xml::{Item, Moment, Parameters, Sequence, Shape, Version, serialize};

/// The name of `CollationA`, among the tests' collations.
pub(super) const COLLATION_A: &str = "COLLATION_A";
/// The name of `Unsplit`, among the tests' collations.
pub(super) const UNSPLIT: &str = "UNSPLIT";

/// [XQueryFO §7.5.1.1] CollationA of the examples of substring matching:
/// `-` and `*` ignorable collation units, every other character a unit
/// compared by its code point.
struct CollationA;

impl CollationA {
    /// `text` without what is ignorable.
    fn kept(text: &str) -> String {
        text.chars().filter(|c| !matches!(c, '-' | '*')).collect()
    }
}

impl Collation for CollationA {
    fn name(&self) -> &str {
        COLLATION_A
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn sort_key(&self, text: &str, _: Pad, out: &mut Vec<u8>) {
        out.extend_from_slice(Self::kept(text).as_bytes());
    }

    fn fold(&self, _: char) -> Option<char> {
        None
    }

    fn weighs(&self, c: char) -> bool {
        !matches!(c, '-' | '*')
    }

    fn level_weights(&self, text: &str, level: usize, out: &mut Vec<u8>) {
        if level == 1 {
            out.extend_from_slice(Self::kept(text).as_bytes());
        }
    }

    fn units(&self, text: &str) -> Option<Vec<Unit>> {
        Some(text.chars().enumerate().map(|(start, c)| Unit { start, weights: vec![Self::kept(&c.to_string()).into_bytes()] }).collect())
    }
}

/// A collation that compares strings by code points but does not say how
/// to split them into collation units [XQueryFO §7.5].
struct Unsplit;

impl Collation for Unsplit {
    fn name(&self) -> &str {
        UNSPLIT
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn sort_key(&self, text: &str, _: Pad, out: &mut Vec<u8>) {
        out.extend_from_slice(text.as_bytes());
    }

    fn fold(&self, _: char) -> Option<char> {
        None
    }

    fn weighs(&self, _: char) -> bool {
        true
    }
}

/// The collations of a database (RULES.md D60), and the two above.
pub(super) fn collations() -> Collations {
    let mut all: Collations = builtin().into_iter().map(|c| (c.name().to_owned(), c)).collect();
    all.insert(COLLATION_A.to_owned(), Arc::new(CollationA));
    all.insert(UNSPLIT.to_owned(), Arc::new(Unsplit));
    all
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
        importable: Vec::new(),
        modules: Vec::new(),
        linked: Vec::new(),
        static_typing: false,
        passed: crate::xquery::Passed::default(),
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
