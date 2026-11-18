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
//! Each `fn:` function is a `LibraryFunction`, found by its expanded QName
//! and arity (`lookup`), called with the focus and dynamic context it
//! reads (`Focus`) and its arguments' values. A function applies the
//! function conversion rules [XQuery §3.1.5] to its arguments itself:
//! each is atomized where its parameter is atomic, an untyped value cast
//! to the parameter's type, a number promoted, so it is called alike
//! whether or not its caller converted them. Each `op:` operator is a
//! plain function of `op`, named after it, that the evaluation of
//! XQuery's operators calls [XQuery Appendix B.2].
//!
//! \[DESIGN\] RULES.md D86: an evaluator's own seam for XQuery functions
//! adapts `LibraryFunction`, so the library is that seam's built-ins. No
//! function recurses on a tree: each walks it by its links. A string or
//! sequence a function would make past `MAX_STRING` or `MAX_ITEMS` is
//! SQLCODE -900 (D18), so no call holds more than an XML value may.

mod accessors;
pub mod args;
mod booleans;
pub mod codes;
mod constructors;
mod decimal;
mod numeric;
mod raise;
mod rounding;
mod uri;

pub mod op;

pub use booleans::effective_boolean_value;
pub use codes::fail;
pub use constructors::{Constructor, construct};
pub use decimal::{DIVISION_SCALE, Decimal, MAX_DIGITS, Rounding};

use std::ops::RangeInclusive;

use crate::error::Result;
use crate::value::datetime::Now;
use crate::xml::{Item, MAX_NODES, MAX_TEXT, Sequence};

/// [XQueryFO §1.2] The namespace of the functions, prefix `fn`.
pub const FN_NAMESPACE: &str = "http://www.w3.org/2005/xpath-functions";
/// [XQueryFO §1.2] The namespace of the data types XQuery adds, prefix
/// `xdt`.
pub const XDT_NAMESPACE: &str = "http://www.w3.org/2005/xpath-datatypes";
/// [XQueryFO §1.2] The namespace of the errors, prefix `err`.
pub const ERR_NAMESPACE: &str = "http://www.w3.org/2005/xqt-errors";
/// [XQueryFO §7.3.1] The Unicode code point collation, which every
/// implementation supports and which is the default collation unless
/// another is given.
pub const CODEPOINT_COLLATION: &str = "http://www.w3.org/2005/xpath-functions/collation/codepoint";

/// \[DESIGN\] The most octets of a string a function makes: a tree's text
/// (RULES.md D86).
pub const MAX_STRING: usize = MAX_TEXT;
/// \[DESIGN\] The most items of a sequence a function makes: a tree's
/// nodes (RULES.md D86).
pub const MAX_ITEMS: usize = MAX_NODES;

/// What a function reads of the evaluation calling it [XQuery §2.1.1,
/// §2.1.2, Appendix C]: the focus, the dynamic context's clock and implicit
/// time zone, and the static context's default collation and base URI.
#[derive(Debug, Clone)]
pub struct Focus {
    /// The context item, if there is one [XQuery §2.1.2].
    pub item: Option<Item>,
    /// The context position, from 1.
    pub position: usize,
    /// The context size.
    pub size: usize,
    /// The current dateTime, the instant the statement reads its clock at
    /// (RULES.md D54), stable within the evaluation [XQueryFO §16.3].
    pub now: Now,
    /// The implicit time zone, in minutes east of UTC.
    pub timezone_minutes: i32,
    /// The URI of the default collation [XQueryFO §7.3.1].
    pub collation: String,
    /// The base URI of the static context, if it has one.
    pub base_uri: Option<String>,
}

impl Focus {
    /// The focus of no context item at `now`, its implicit time zone
    /// `now`'s, its default collation the code point collation and no base
    /// URI, as [XQuery Appendix C] gives them by default.
    #[must_use]
    pub fn new(now: Now) -> Focus {
        Focus {
            item: None,
            position: 0,
            size: 0,
            now,
            timezone_minutes: i32::from(now.zone),
            collation: CODEPOINT_COLLATION.to_owned(),
            base_uri: None,
        }
    }

    /// It with the context item `item`, at `position` of `size`.
    #[must_use]
    pub fn at(&self, item: Item, position: usize, size: usize) -> Focus {
        Focus { item: Some(item), position, size, ..self.clone() }
    }
}

/// A function of the library [XQueryFO §1.4]: its expanded QName, the
/// numbers of arguments it takes, and its body.
pub trait LibraryFunction: Send + Sync {
    /// Its expanded QName: its namespace URI and its local name.
    fn name(&self) -> (&'static str, &'static str);
    /// How many arguments it takes, least to most.
    fn arity(&self) -> RangeInclusive<usize>;
    /// Its value of `args` in `focus`.
    ///
    /// # Errors
    ///
    /// The XQuery error \[XQueryFO\] names, SQLSTATE 10000; SQLCODE -900 of
    /// a value past turborel's limits.
    fn call(&self, focus: &Focus, args: &[Sequence]) -> Result<Sequence>;
}

/// The body of a built-in function.
type Body = fn(&Focus, &[Sequence]) -> Result<Sequence>;

/// A built-in function: an entry of a section's table.
#[derive(Debug, Clone, Copy)]
pub struct Builtin {
    namespace: &'static str,
    local: &'static str,
    least: usize,
    most: usize,
    body: Body,
}

impl Builtin {
    /// The function `local` of the namespace `fn` taking `least` to `most`
    /// arguments, `body` its value.
    const fn function(local: &'static str, least: usize, most: usize, body: Body) -> Builtin {
        Builtin { namespace: FN_NAMESPACE, local, least, most, body }
    }
}

impl LibraryFunction for Builtin {
    fn name(&self) -> (&'static str, &'static str) {
        (self.namespace, self.local)
    }

    fn arity(&self) -> RangeInclusive<usize> {
        self.least..=self.most
    }

    fn call(&self, focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
        (self.body)(focus, args)
    }
}

/// Every section's functions.
const SECTIONS: [&[Builtin]; 6] =
    [&accessors::FUNCTIONS, &raise::FUNCTIONS, &constructors::FUNCTIONS, &rounding::FUNCTIONS, &uri::FUNCTIONS, &booleans::FUNCTIONS];

/// Every function of the library: each section's, then the constructors
/// [XQueryFO §5.1].
pub fn all() -> impl Iterator<Item = &'static dyn LibraryFunction> {
    let builtins = SECTIONS.into_iter().flatten().map(|f| -> &'static dyn LibraryFunction { f });
    builtins.chain(constructors::CONSTRUCTORS.iter().map(|c| -> &'static dyn LibraryFunction { c }))
}

/// The function of the expanded QName `namespace` and `local` that takes
/// `arity` arguments, if the library has it [XQuery §3.1.5].
#[must_use]
pub fn lookup(namespace: &str, local: &str, arity: usize) -> Option<&'static dyn LibraryFunction> {
    all().find(|f| f.name() == (namespace, local) && f.arity().contains(&arity))
}

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
