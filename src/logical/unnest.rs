//! [ISO/IEC 9075-2:2023 §7.6 SR8] A `<collection derived table>`, and the
//! JSON tables and GRAPH_TABLEs planned as one, laterally over the rows of
//! its input.

use super::{Cond, Expr, GraphMatch};

/// [ISO/IEC 9075-2:2023 §7.6 SR8] A `<collection derived table>` over the
/// rows of `input`, of plans `P`: each row of it is a row of `input`, then
/// one column for each element, or one for each field of an element of
/// row type, then the ordinal if WITH ORDINALITY is written.
#[derive(Debug, Clone)]
pub struct Unnest<P> {
    /// The `<collection value expression>`s, over the columns of `input`.
    pub collections: Vec<Expr>,
    /// Of each collection, the degree of its element type if it is a row
    /// type, whose fields are columns of their own [SR8m ii, SR8n].
    pub spread: Vec<Option<usize>>,
    /// WITH ORDINALITY: the ordinal is a column after the elements.
    pub ordinality: bool,
    /// [ISO/IEC 9075-2:2023 §7.10 GR1d] Of the second operand of a LEFT
    /// join, its `<join condition>`, over the columns of its rows: a row of
    /// `input` that none of them makes true is kept, its own columns null.
    pub outer: Option<Cond>,
    /// Its own columns' display names, after `input`'s.
    pub names: Vec<String>,
    /// The rows each gives the rows of its collections.
    pub input: P,
    /// [ISO/IEC 9075-2:2023 §7.11] Of a `<JSON table>`, which is planned
    /// as a collection derived table is, laterally: the table, its
    /// operands being `collections` (`JsonTable::open`), its columns its
    /// own, and `spread` and `ordinality` none.
    pub json: Option<std::sync::Arc<crate::json::JsonTable>>,
    /// \[EXTENSION\] Of a GRAPH_TABLE, which is planned as a lateral
    /// derived table is: the bound table, its operands being
    /// `collections`, its columns its COLUMNS (`logical/graph.rs`).
    pub graph: Option<std::sync::Arc<GraphMatch>>,
}

impl<P> Unnest<P> {
    /// It apart from its input, and its input.
    pub fn split(self) -> (Unnest<()>, P) {
        let Unnest { collections, spread, ordinality, outer, names, input, json, graph } = self;
        (Unnest { collections, spread, ordinality, outer, names, input: (), json, graph }, input)
    }

    /// It over `f` of its input, the rest copied.
    pub fn with_input<Q>(&self, f: impl FnOnce(&P) -> Q) -> Unnest<Q> {
        Unnest {
            collections: self.collections.clone(),
            spread: self.spread.clone(),
            ordinality: self.ordinality,
            outer: self.outer.clone(),
            names: self.names.clone(),
            input: f(&self.input),
            json: self.json.clone(),
            graph: self.graph.clone(),
        }
    }

    /// The number of its own columns: those of the elements, and the
    /// ordinal's.
    pub fn width(&self) -> usize {
        let elements = self.spread.iter().map(|s| s.unwrap_or(1)).sum::<usize>();
        let json = self.json.as_ref().map_or(0, |j| j.width);
        let graph = self.graph.as_ref().map_or(0, |g| g.columns.len());
        elements.saturating_add(usize::from(self.ordinality)).saturating_add(json).saturating_add(graph)
    }
}

impl Unnest<()> {
    /// The same collection derived table over `input`.
    pub fn over<Q>(self, input: Q) -> Unnest<Q> {
        let Unnest { collections, spread, ordinality, outer, names, input: (), json, graph } = self;
        Unnest { collections, spread, ordinality, outer, names, input, json, graph }
    }
}
