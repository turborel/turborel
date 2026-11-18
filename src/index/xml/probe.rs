//! \[EXTENSION\] RULES.md D86: an XML index asked by the planner
//! (`Probe::Xml`): of a query's lookup, the rows it gives, each fetched
//! and checked by the condition it was made of; or, of an index that
//! answers of a row from its entries alone (`Index::xml_answers`), the
//! rows it answers, only those it may hold of checked, by the query's own
//! check (`XmlQuery::matches`).

use super::lookup::XmlQuery;
use crate::index::Probe;

/// \[EXTENSION\] RULES.md D86: an XML index's query, as a probe asks it.
#[derive(Debug, Clone)]
pub struct XmlProbe {
    /// The query: its lookup, and a filter of an ancestor, which an index
    /// that does not answer leaves to the condition's check.
    pub query: XmlQuery,
    /// Whether the index answers it (`Index::xml_answers`): the query is
    /// the whole condition, and a row it holds of is kept unchecked.
    pub answered: bool,
}

impl Probe {
    /// Whether its rows cannot be told by their keys: a function lookup's
    /// or an XML index's, read as of every row (RULES.md D34, D86).
    #[must_use]
    pub fn opaque(&self) -> bool {
        matches!(self, Probe::Predicate { .. } | Probe::Xml(_))
    }
}
