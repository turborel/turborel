//! [ISO/IEC 9075-14:2006 draft §11.3] The `<XML lexically scoped
//! options>` in scope as an expression is bound, the innermost first: of
//! an XMLELEMENT or XMLFOREST it is in [§6.14 SR1, §6.15 SR1]; of each
//! `<with clause>` whose `<query expression>` it is in [§7.2 SR2, SR3];
//! of its statement [§14.3 to §14.7] or the definition it is the
//! condition or expression of [§12.1, §12.2, §12.4]; and of each compound
//! statement it is in [§15.1].
//!
//! A binder keeps the declarations of the XMLELEMENTs and XMLFORESTs it
//! is binding (`Scopes::levels`). The options of a `<with clause>` are
//! kept with its query names (`Queries::options`), which its context
//! links to those of the clauses around it. A subquery searches its own,
//! then those of the query it is a subquery of; a derived table and a
//! `<with list element>` are subqueries of the query around theirs, and
//! their context holds the clauses between. The options of the statement
//! or definition, and of the compound statements it is in, are of its
//! parameters (`Params::xml_options`, `RoutineScope::xml`), searched
//! last.

use std::cell::RefCell;

use super::super::{Binder, Queries};
use crate::ast::LexicallyScopedOptions;
use crate::functions::{BinaryEncoding, IMPLICIT_BINARY_ENCODING};

/// The declarations of the XMLELEMENTs and XMLFORESTs a binder is
/// binding, the innermost last.
#[derive(Default)]
pub(in crate::binder) struct Scopes {
    /// Each, as options of no encoding.
    pub(in crate::binder) levels: RefCell<Vec<LexicallyScopedOptions>>,
}

/// Whether `a` is `b`.
fn same(a: &Queries<'_>, b: Option<&Queries<'_>>) -> bool {
    b.is_some_and(|b| std::ptr::eq(a, b))
}

impl Binder<'_> {
    /// What `find` gives of the first options in scope it gives anything
    /// of, the innermost first.
    pub(super) fn in_xml_scope<T>(&self, find: impl Fn(&LexicallyScopedOptions) -> Option<T>) -> Option<T> {
        let mut binder = self;
        loop {
            if let Some(found) = binder.xml.levels.borrow().iter().rev().find_map(&find) {
                return Some(found);
            }
            // The clauses around it down to those of the query it is a
            // subquery of, which that query searches after its own.
            let around = binder.outer.and_then(|o| o.binder().queries);
            let mut clause = binder.queries;
            while let Some(c) = clause.filter(|c| !same(c, around)) {
                if let Some(found) = c.options().and_then(&find) {
                    return Some(found);
                }
                clause = c.parent();
            }
            let Some(outer) = binder.outer else { break };
            binder = outer.binder();
        }
        let statement = self.params.xml_options().and_then(&find);
        statement.or_else(|| self.params.routine_scope()?.xml.iter().rev().find_map(&find))
    }

    /// [§6.6 SR1, §6.14 GR1, §6.15 GR1, §6.19 GR3] The `<XML binary
    /// encoding>` with innermost scope that contains what it binds now;
    /// else the implementation-defined one, HEX.
    pub(super) fn binary_encoding(&self) -> BinaryEncoding {
        self.in_xml_scope(|o| o.binary).unwrap_or(IMPLICIT_BINARY_ENCODING)
    }
}
