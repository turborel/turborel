//! What a statement gives a call that reads it (RULES.md D87).

use super::Subqueries;
use crate::error::Result;
use crate::functions::CallContext;

/// What a statement gives a call that reads it (`ScalarFunction::call_in`):
/// its runner's.
pub(super) struct Context<'s>(pub(super) &'s dyn Subqueries);

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
