//! \[EXTENSION\] The table function HYBRID_SEARCH (the vector design
//! document §11.2), as read: the binder makes it the query it stands for
//! (`binder/hybrid_search.rs`), as it needs the index it names.

use std::collections::HashSet;

/// [§11.3] How the two lists of candidates are fused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fusion {
    /// Reciprocal rank fusion: each list adds its weight over K and the
    /// document's rank in it.
    Rrf,
    /// Weighted fusion: each list adds its weight times the document's
    /// score in it, normalized min-max over its candidates.
    Weighted,
}

/// A value argument of HYBRID_SEARCH, as written.
#[derive(Debug, Clone)]
pub struct SearchValue {
    /// Its source text, read again as the binder writes the query.
    pub text: String,
    /// Whether it is one `<character string literal>`, and its value if so.
    pub literal: Option<String>,
}

/// \[EXTENSION\] `HYBRID_SEARCH ( <hybrid argument> [{, <hybrid
/// argument>}...] ) [AS] <correlation name> [(<derived column list>)]`.
#[derive(Debug, Clone)]
pub struct HybridSearch {
    /// INDEX: the hybrid index searched.
    pub index: String,
    /// QUERY_TEXT: the words searched for.
    pub text: SearchValue,
    /// QUERY_VECTOR: the vector searched for. Of none, an index that
    /// embeds its text embeds QUERY_TEXT by its model.
    pub vector: Option<SearchValue>,
    /// Whether QUERY_VECTOR is written before QUERY_TEXT, so the binder
    /// writes the query's dynamic parameters in their order.
    pub vector_first: bool,
    /// TOP_N: the rows it gives.
    pub top_n: u32,
    /// CANDIDATES: the rows each list holds, TOP_N times 4 if none [R11.1].
    pub candidates: Option<u32>,
    /// FUSION.
    pub fusion: Fusion,
    /// VECTOR_WEIGHT and TEXT_WEIGHT, 1 each if not written.
    pub weights: (f64, f64),
    /// RRF_K: the K of reciprocal rank fusion, 60 if not written.
    pub rrf_k: u32,
    /// Its `<correlation name>`, HYBRID_SEARCH if none is written.
    pub correlation: String,
    /// Its `<derived column list>`, if written.
    pub columns: Option<Vec<String>>,
    /// The `<dynamic parameter specification>`s before its first value
    /// argument, of a statement being prepared; none of another.
    pub dynamic: Option<usize>,
    /// The parameters of the procedure it is in, which its arguments may
    /// name.
    pub params: HashSet<String>,
    /// Whether its statement takes the function call syntax of registered
    /// functions.
    pub extensions: bool,
    /// Where it begins in the statement.
    pub at: usize,
}
