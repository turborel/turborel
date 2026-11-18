//! What a statement gives a call that reads it (RULES.md D87, D106).

use super::Subqueries;
use crate::error::Result;
use crate::functions::{CallContext, ScalarRef};
use crate::value::Value;

/// A call of `function`, of the values `args`, that reads the statement
/// `subqueries` runs: an XQuery evaluation given the statement, so it is
/// cancelled as it goes (`ScalarFunction::in_statement`, RULES.md D44), or
/// one given its context (`ScalarFunction::uses_context`, RULES.md D87).
///
/// # Errors
///
/// As the call's.
pub(super) fn reading(function: &ScalarRef, args: &[Value], subqueries: &dyn Subqueries) -> Result<Value> {
    if function.0.in_statement() {
        return function.0.call_in_statement(args, subqueries);
    }
    function.0.call_in(args, &Context(subqueries))
}

/// What a statement gives a call that reads it (`ScalarFunction::call_in`):
/// its runner's.
struct Context<'s>(&'s dyn Subqueries);

impl CallContext for Context<'_> {
    fn temp_lobs(&self) -> Option<&dyn crate::value::lob::TempLobs> {
        self.0.temp_lobs()
    }

    fn ranking_statistics(&self, index: &str) -> Result<Option<std::sync::Arc<dyn crate::fulltext::rank::Statistics>>> {
        self.0.ranking_statistics(index)
    }

    fn ranking(&self, name: &str) -> Option<std::sync::Arc<dyn crate::fulltext::rank::Ranking>> {
        match self.0.state() {
            Some(state) => state.rankings.named(name),
            None => crate::fulltext::rank::named(&crate::fulltext::rank::built_in(), name),
        }
    }
}
