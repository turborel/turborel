//! \[EXTENSION\] How a `<fetch first clause>`'s rows are found: exactly, as
//! the standard's sort finds them, or by an approximate index (the vector
//! design document §9.2, R9.2 to R9.5, §13 R13.1, R13.2 and R13.4).
//! APPROX is a request, never a guarantee: a query of no usable index
//! runs exact, and an approximate read that finds too few rows widens its
//! search and then reads the whole table (RULES.md D-APPROX).

/// \[EXTENSION\] The search a `<fetch first clause>` asks for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Search {
    /// Neither APPROX nor EXACT written: exact, unless the session's
    /// VECTOR.APPROX_DEFAULT is ON (R13.1).
    #[default]
    Unstated,
    /// FETCH EXACT: never an approximate index (§9.1).
    Exact,
    /// FETCH APPROX, with the accuracy and parameters written (§9.2).
    Approximate(SearchTarget),
}

/// \[EXTENSION\] What `WITH TARGET ACCURACY` asks of an approximate search
/// (§9.2, R9.4): each none of none written, the index's own then holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchTarget {
    /// `WITH TARGET ACCURACY n`: the share of the true nearest rows to
    /// find, a percentage from 1 to 100.
    pub accuracy: Option<u8>,
    /// `EFSEARCH n` or `SEARCH_LIST n`: the candidates a graph search
    /// keeps, HNSW's ef and DiskANN's L.
    pub search_list: Option<u32>,
    /// `BEAM_WIDTH n`: the nodes whose neighbours DiskANN reads together.
    pub beam_width: Option<u32>,
    /// `NEIGHBOR PARTITION PROBES n`: the lists IVF reads.
    pub probes: Option<u32>,
}

/// \[EXTENSION\] Where an approximate read applies its WHERE condition
/// (R13.2), chosen by the condition's estimated selectivity unless a hint
/// names one (R13.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// The condition first, by the ordinary plan and its indexes, and the
    /// exact distance of the rows it keeps: no approximate index.
    Pre,
    /// The index asks of each candidate whether its row meets the
    /// condition as it searches (`RowFilter`), so it widens past those
    /// that do not.
    In,
    /// The index searches for more candidates than asked, as many as the
    /// condition is estimated to keep the rows asked of, then filters.
    Post,
}

impl Strategy {
    /// It as EXPLAIN shows it.
    pub fn word(self) -> &'static str {
        match self {
            Strategy::Pre => "pre-filter",
            Strategy::In => "in-filter",
            Strategy::Post => "post-filter",
        }
    }
}

/// \[EXTENSION\] The hints of R13.4 written after a `<fetch first clause>`.
/// Each table is named by its `<identifier>`, and a hint naming a table
/// the query does not read is no error, as hints never are.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hints {
    /// `VECTOR_INDEX(t i)`: the table and the index its read takes.
    pub index: Option<(String, String)>,
    /// `NO_VECTOR_INDEX(t)`: the table read by no approximate index.
    pub no_index: Option<String>,
    /// `VECTOR_PREFILTER`, `VECTOR_INFILTER` or `VECTOR_POSTFILTER`.
    pub filter: Option<Strategy>,
    /// `VECTOR_WAIT_FRESH`: the read sees every committed change. Each
    /// built-in index is changed as its table is, with no delta store
    /// merged later, so every read already does.
    pub wait_fresh: bool,
}

impl Hints {
    /// Whether none is written.
    pub fn is_empty(&self) -> bool {
        *self == Hints::default()
    }

    /// Whether they keep the table named `table` from every approximate
    /// index.
    pub fn bars(&self, table: &str) -> bool {
        self.no_index.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(table))
    }

    /// The index they name for the table named `table`, if any.
    pub fn index_of(&self, table: &str) -> Option<&str> {
        self.index.as_ref().filter(|(t, _)| t.eq_ignore_ascii_case(table)).map(|(_, i)| i.as_str())
    }
}
