//! \[EXTENSION\] RULES.md D106: an XML index asked by the planner
//! (`Probe::Xml`): of queries' lookups, the rows they give, each fetched
//! and checked by the condition it was made of; or, of an index that
//! answers of a row from its entries alone (`Index::xml_answers`), the
//! rows it answers, only those it may hold of checked, by the query's own
//! check (`XmlQuery::matches`).

use super::lookup::{XmlLookup, XmlQuery};
use crate::index::Probe;

/// \[DESIGN\] How many lookups a probe reads at most: past them, a probe of
/// every lookup's rows reads the first alone, which holds them. The value
/// is no measurement: each is read whole, of each row once, and a few hold
/// what more would add.
pub const MAX_LOOKUPS: usize = 16;

/// \[EXTENSION\] RULES.md D106: an XML index's queries, as a probe asks
/// them.
#[derive(Debug, Clone)]
pub struct XmlProbe {
    /// The queries, conjuncts of a table's filter, each holding of its
    /// rows: of a probe answered, one.
    pub queries: Vec<XmlQuery>,
    /// Whether the index answers its one query (`Index::xml_answers`): the
    /// query is the whole condition, and a row it holds of is kept
    /// unchecked.
    pub answered: bool,
    /// The lookups read, of the queries' (`XmlQuery::lookups`), as the
    /// planner chose them: the cheapest first.
    pub lookups: Vec<XmlLookup>,
    /// Whether its rows are those any lookup gives, the queries' rows and
    /// the rows they may raise an error of among them; else those every
    /// lookup gives.
    pub any: bool,
}

impl Probe {
    /// Whether its rows cannot be told by their keys: a function lookup's
    /// or an XML index's, read as of every row (RULES.md D34, D106).
    #[must_use]
    pub fn opaque(&self) -> bool {
        matches!(self, Probe::Predicate { .. } | Probe::Xml(_))
    }
}
