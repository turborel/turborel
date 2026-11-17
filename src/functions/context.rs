//! \[EXTENSION\] What a call is given beside its arguments: the statement's
//! context (`ScalarFunction::call_in`) and a query's ARRAY appended to them
//! (`ScalarFunction::appended_query`), as full-text search uses them
//! (RULES.md D87).

use std::sync::Arc;

use crate::error::Result;

/// \[EXTENSION\] What a statement gives a call beside its arguments
/// (`ScalarFunction::call_in`, RULES.md D87).
pub trait CallContext {
    /// Where the call's temporary large objects are kept, if anywhere.
    fn temp_lobs(&self) -> Option<&dyn crate::value::lob::TempLobs>;
    /// The ranking statistics of index `index` (`Index::ranking_statistics`),
    /// taken once in the statement, so every call of it reads the same; none
    /// if there is no such index or it ranks nothing.
    ///
    /// # Errors
    ///
    /// As taking them.
    fn ranking_statistics(&self, index: &str) -> Result<Option<Arc<dyn crate::fulltext::rank::Statistics>>>;
    /// The ranking of full-text search named `name`, built in or registered
    /// (`Database::register_ranking`); BM25 alone where no database is.
    fn ranking(&self, name: &str) -> Option<Arc<dyn crate::fulltext::rank::Ranking>>;
}

/// A query whose ARRAY a call is given as its last argument
/// (`ScalarFunction::appended_query`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendedQuery {
    /// Its text, a `<query expression>` of one column, as the binder reads
    /// it.
    pub query: String,
    /// The tables it reads, each its schema's name and its own: it is
    /// appended only if they all are there.
    pub tables: Vec<(String, String)>,
}
