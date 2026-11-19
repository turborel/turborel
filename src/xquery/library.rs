//! The functions and operators of XQuery 1.0 \[XQueryFO\], the W3C
//! Candidate Recommendation of 3 November 2005 that [ISO/IEC 9075-14:2006
//! draft §2.2] names, as a library over XML values (`xml::Sequence`,
//! `xml::Item`, `xml::Atomic`), one file for each of its sections:
//! accessors (§2), the error and trace functions (§3, §4), constructors
//! (§5), numerics (§6), strings (§7), URIs (§8), booleans (§9), durations,
//! dates and times (§10), QNames (§11), binary values and NOTATION (§12,
//! §13), nodes (§14), sequences (§15) and the context (§16). Casting
//! (§17) is `xml::cast`.
//!
//! Each `fn:` function is a `Builtin`, a row of its section's table: its
//! local name, the numbers of arguments it takes and its body, which the
//! library of the evaluation (`functions::Library`) makes the built-in
//! functions of, one `XQueryFunction` of each arity. A body is called with
//! the dynamic context of the call (`functions::Context`) and its
//! arguments' values. It applies the function conversion rules [XQuery
//! §3.1.5] to its arguments itself: each is atomized where its parameter
//! is atomic, an untyped value cast to the parameter's type, a number
//! promoted, so the parameters it declares are each `item()*`. Each `op:`
//! operator is a plain function of `op`, named after it, that the
//! evaluation of XQuery's operators calls [XQuery Appendix B.2].
//!
//! \[DESIGN\] RULES.md D106: no function recurses on a tree: each walks it
//! by its links. A string or sequence a function would make past
//! `MAX_STRING` or `MAX_ITEMS` is SQLCODE -900 (D18), so no call holds
//! more than an XML value may; a function whose work grows faster than
//! its arguments looks whether the statement was cancelled as it goes
//! (D44).

mod accessors;
mod aggregates;
pub mod args;
mod binary;
mod booleans;
mod calendar;
mod comparison;
mod constructors;
mod context;
mod decimal;
mod deep_equal;
mod durations;
mod escape;
mod generate;
mod moments;
mod node_sets;
mod nodes;
mod normalize;
mod numeric;
mod patterns;
mod qnames;
mod raise;
mod rounding;
mod sequences;
mod strings;
mod substrings;
mod unicode;
mod uri;

pub mod op;

pub use booleans::effective_boolean_value;
pub use comparison::{value_comparison, value_equal, value_order};
pub use decimal::{DIVISION_SCALE, Decimal, MAX_DIGITS, Rounding};
pub use deep_equal::sequences_deep_equal;

use super::functions::Context;
use crate::error::Result;
use crate::xml::{MAX_NODES, MAX_TEXT, Sequence};

/// [XQueryFO §1.2] The namespace of the errors, prefix `err`.
pub const ERR_NAMESPACE: &str = "http://www.w3.org/2005/xqt-errors";

/// \[DESIGN\] The most octets of a string a function makes: a tree's text
/// (RULES.md D106).
pub const MAX_STRING: usize = MAX_TEXT;
/// \[DESIGN\] The most items of a sequence a function makes: a tree's
/// nodes (RULES.md D106).
pub const MAX_ITEMS: usize = MAX_NODES;

/// The body of a built-in function: its value of its arguments in the
/// dynamic context of its call.
pub(super) type Body = fn(&Context<'_>, &[Sequence]) -> Result<Sequence>;

/// A built-in function of the namespace `fn`: a row of a section's table.
#[derive(Debug, Clone, Copy)]
pub(super) struct Builtin {
    /// Its local name.
    pub(super) local: &'static str,
    /// The fewest arguments it takes.
    pub(super) least: usize,
    /// The most arguments it takes, `usize::MAX` of any number.
    pub(super) most: usize,
    /// What it computes.
    pub(super) body: Body,
}

impl Builtin {
    /// The function `local` taking `least` to `most` arguments, `body` its
    /// value.
    const fn function(local: &'static str, least: usize, most: usize, body: Body) -> Builtin {
        Builtin { local, least, most, body }
    }
}

/// Every section's functions, a line for each section's table, in the
/// order of the sections of \[XQueryFO\].
const SECTIONS: &[&[Builtin]] = &[
    &accessors::FUNCTIONS,
    &raise::FUNCTIONS,
    &constructors::FUNCTIONS,
    &rounding::FUNCTIONS,
    &strings::FUNCTIONS,
    &normalize::FUNCTIONS,
    &escape::FUNCTIONS,
    &substrings::FUNCTIONS,
    &patterns::FUNCTIONS,
    &uri::FUNCTIONS,
    &booleans::FUNCTIONS,
    &durations::FUNCTIONS,
    &moments::FUNCTIONS,
    &qnames::FUNCTIONS,
    &nodes::FUNCTIONS,
    &sequences::FUNCTIONS,
    &deep_equal::FUNCTIONS,
    &aggregates::FUNCTIONS,
    &generate::FUNCTIONS,
    &context::FUNCTIONS,
];

/// Every function of the library, each section's in turn.
pub(super) fn all() -> impl Iterator<Item = &'static Builtin> {
    SECTIONS.iter().copied().flatten()
}

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
