//! \[EXTENSION\] The values a statement gives the pseudo-columns of each
//! row it writes of a node or edge table (§4.2, R5.1): an inserted row's
//! identity, the next value of its table's generator written as the
//! identity's text, and an edge's endpoints, each the text of a node of a
//! node table of its schema, written again in the one form an identity
//! has, so equal texts are one node.

use std::sync::Arc;

use crate::catalog::{EDGE_ID, ElementId, FROM_ID, NODE_ID, NativeKind, QualifiedName, SequenceDesc, TO_ID, TableDesc};
use crate::db::{State, api};
use crate::error::{Code, Result, SqlError};
use crate::storage::Row;
use crate::value::Value;

/// What a statement gives the pseudo-columns of each row of a node or
/// edge table it writes.
#[derive(Debug, Clone)]
pub(in crate::db) struct Native {
    /// The table.
    table: QualifiedName,
    /// Its identity column's position, whether it is a node table's, and
    /// its generator, if each row takes its next value: an INSERT's.
    draw: Option<(usize, bool, Arc<SequenceDesc>)>,
    /// The positions of `$FROM_ID` and `$TO_ID`, of an edge table.
    ends: Vec<usize>,
}

impl Native {
    /// Of table `desc`, if it is a node or edge table: what each row it
    /// writes is given, drawing an identity if `inserting`.
    pub(in crate::db) fn of(desc: &TableDesc, inserting: bool) -> Option<Native> {
        let kind = desc.native()?;
        let (column, node) = match kind {
            NativeKind::Node => (NODE_ID, true),
            NativeKind::Edge => (EDGE_ID, false),
        };
        let draw = desc
            .pseudo_column(column)
            .and_then(|at| Some((at, node, Arc::new(desc.columns.get(at)?.graph_column()?.sequence()?.clone()))))
            .filter(|_| inserting);
        let ends = [FROM_ID, TO_ID].iter().filter_map(|n| desc.pseudo_column(n)).collect();
        Some(Native { table: desc.name.clone(), draw, ends })
    }

    /// The position of the identity it gives each row, if it gives one.
    pub(in crate::db) fn drawn(&self) -> Option<usize> {
        self.draw.as_ref().map(|(at, ..)| *at)
    }

    /// `row` given its identity, if each row is, and its endpoints written
    /// as their identities' texts.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200H if the generator is exhausted; SQLCODE -201 (GRF-0053)
    /// if an endpoint is no node's identity of a node table of the table's
    /// schema.
    pub(in crate::db) fn give(&self, st: &State, row: &mut Row) -> Result<()> {
        if let Some((at, node, seq)) = &self.draw {
            let Value::Exact(next) = st.next_value(seq)? else { return Err(api("[DESIGN] a generator gave no integer")) };
            let id = ElementId { node: *node, table: self.table.clone(), id: next.units };
            put(row, *at, Value::Char(id.text()))?;
        }
        for &at in &self.ends {
            let Some(Value::Char(text)) = row.get(at) else { continue };
            let id = self.endpoint(st, text)?;
            put(row, at, Value::Char(id.text()))?;
        }
        Ok(())
    }

    /// The node `text` is the identity of.
    ///
    /// # Errors
    ///
    /// As `give`.
    fn endpoint(&self, st: &State, text: &str) -> Result<ElementId> {
        let id = ElementId::parse(text).filter(|id| id.node && id.table.auth == self.table.auth);
        let node = id.filter(|id| st.catalog.table(&id.table).is_some_and(|t| t.native() == Some(NativeKind::Node)));
        node.ok_or_else(|| {
            SqlError::new(
                Code::Constraint,
                format!(
                    "[EXTENSION] GRF-0053 an edge of {} joins nodes of node tables of schema {} (§4.2), and {text} is the identity of none",
                    self.table, self.table.auth
                ),
            )
        })
    }
}

/// `value` in the column at `at` of `row`.
fn put(row: &mut Row, at: usize, value: Value) -> Result<()> {
    let slot = row.get_mut(at).ok_or_else(|| api(format!("[DESIGN] a row has no column {at}")))?;
    *slot = value;
    Ok(())
}
