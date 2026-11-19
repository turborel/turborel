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
    /// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.3] The spatial
    /// reference systems the statement's catalog defines, beside those
    /// supplied; none where no database runs the call, as a constant is
    /// folded, where a call reading a system defined must wait for its
    /// statement.
    fn spatial(&self) -> Option<&crate::catalog::SpatialCatalog>;
    /// \[EXTENSION\] The clock of the statement's database, and the last
    /// version 7 UUID it made (RULES.md D183); none where no database
    /// runs the call.
    fn uuid_clock(&self) -> Option<(Arc<dyn crate::clock::Clock>, Arc<crate::value::types::uuid::generate::Ordered>)> {
        None
    }
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
    /// The columns the first of its tables must have: it is appended only
    /// if that table has them all.
    pub columns: Vec<String>,
}
