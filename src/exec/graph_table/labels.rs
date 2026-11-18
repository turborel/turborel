//! \[EXTENSION\] A GRAPH_TABLE's label tests read through label indexes
//! (RULES.md D115). Each element table with one has its rows numbered
//! by it; numbered after those of the tables before it, at a word's
//! boundary, they are one space of the graph's elements, and each label
//! is a bitmap of the elements carrying it, across every table sharing
//! it. A reached element's labels are its bits, and the starts of a path
//! pattern are the elements of a table whose bits its first node
//! pattern's label expression holds of, each seen by the reader, read
//! without its row's columns.
//!
//! A label is of a whole element table in a graph defined over tables,
//! so a bitmap of it holds a table's elements all or none: the label
//! tests give what the tables' labels give, and the gain is the starts
//! found without a scan of their rows.

use std::sync::Arc;

use super::store::Store;
use crate::db::State;
use crate::error::Result;
use crate::graph::indexes::{MemberSet, WORD_BITS, place};
use crate::graph::program::LabelExpr;
use crate::graph::view::{ElementId, LabelId};
use crate::logical::{GraphMatch, PathShape, PatternShape, TestShape};
use crate::storage::RowId;

/// \[EXTENSION\] The label bitmaps of a GRAPH_TABLE's graph.
#[derive(Default)]
pub(super) struct GraphLabels {
    /// Of each element table, its members and its first number, if a
    /// label index numbers it.
    tables: Vec<Option<(Arc<MemberSet>, usize)>>,
    /// Of each label, the elements carrying it, by bit.
    bitmaps: Vec<Vec<u64>>,
}

/// The members of element table `t` of `graph`, if a label index numbers them.
fn members_of(db: &State, graph: &GraphMatch, t: usize) -> Result<Option<Arc<MemberSet>>> {
    let Some(name) = graph.tables.get(t).and_then(|s| s.labels_index.as_deref()) else { return Ok(None) };
    let Some(members) = db.index_of(name).ok().and_then(|ix| Arc::clone(ix).members()) else { return Ok(None) };
    members.members()
}

impl GraphLabels {
    /// The bitmaps of `graph`'s labels, of the label indexes of its tables.
    ///
    /// # Errors
    ///
    /// An index's storage fails.
    pub(super) fn new(db: &State, graph: &GraphMatch) -> Result<GraphLabels> {
        let (mut tables, mut next) = (Vec::new(), 0_usize);
        for t in 0..graph.tables.len() {
            let members = members_of(db, graph, t)?;
            tables.push(members.map(|m| {
                let base = next;
                next = next.saturating_add(m.len().div_ceil(WORD_BITS).saturating_mul(WORD_BITS));
                (m, base)
            }));
        }
        let mut bitmaps = vec![vec![0_u64; next.div_ceil(WORD_BITS)]; graph.labels.len()];
        for (source, at) in graph.tables.iter().zip(&tables) {
            let Some((members, base)) = at else { continue };
            for &label in &source.labels {
                let Some(bitmap) = bitmaps.get_mut(usize::try_from(label).unwrap_or(usize::MAX)) else { continue };
                let words = bitmap.iter_mut().skip(base.checked_div(WORD_BITS).unwrap_or_default());
                for (word, bits) in words.zip(members.bitmap()) {
                    *word |= bits;
                }
            }
        }
        Ok(GraphLabels { tables, bitmaps })
    }

    /// The number of `element` among the graph's, if a label index numbers it.
    fn number(&self, element: ElementId) -> Option<usize> {
        let (members, base) = self.tables.get(usize::try_from(element.table).ok()?)?.as_ref()?;
        base.checked_add(usize::try_from(members.id(element.row)?).ok()?)
    }

    /// Whether the element of number `n` carries `label`.
    fn carries(&self, n: usize, label: usize) -> bool {
        let (word, bit) = place(n);
        self.bitmaps.get(label).and_then(|b| b.get(word)).is_some_and(|w| w & bit != 0)
    }

    /// Pushes onto `out` the labels of `element`, its bits: whether a
    /// label index numbers it, else it pushes none.
    pub(super) fn labels_of(&self, element: ElementId, out: &mut Vec<LabelId>) -> bool {
        let Some(n) = self.number(element) else { return false };
        let carried = (0..self.bitmaps.len()).filter(|&l| self.carries(n, l));
        out.extend(carried.filter_map(|l| u32::try_from(l).ok()));
        true
    }

    /// Whether `expr` holds of the element of number `n` [GS §6.2].
    fn holds(&self, expr: &LabelExpr, n: usize) -> bool {
        match expr {
            LabelExpr::Label(l) => self.carries(n, usize::try_from(*l).unwrap_or(usize::MAX)),
            LabelExpr::Wildcard => (0..self.bitmaps.len()).any(|l| self.carries(n, l)),
            LabelExpr::Not(e) => !self.holds(e, n),
            LabelExpr::And(a, b) => self.holds(a, n) && self.holds(b, n),
            LabelExpr::Or(a, b) => self.holds(a, n) || self.holds(b, n),
        }
    }

    /// The rows of element table `t` numbered in use whose bits `expr`
    /// holds of, in the order of the rows; none if no label index numbers it.
    fn candidates(&self, t: usize, expr: Option<&LabelExpr>) -> Option<Vec<RowId>> {
        let (members, base) = self.tables.get(t)?.as_ref()?;
        let ids = (0..u32::try_from(members.len()).unwrap_or(u32::MAX)).filter(|&id| members.holds(id));
        let held = ids.filter(|&id| {
            let n = base.saturating_add(usize::try_from(id).unwrap_or(usize::MAX));
            expr.is_none_or(|e| self.holds(e, n))
        });
        let mut rows: Vec<RowId> = held.filter_map(|id| members.row(id)).collect();
        rows.sort_unstable();
        Some(rows)
    }
}

/// The node pattern every match of `p` begins with, if one does.
fn first_vertex(p: &PatternShape) -> Option<&TestShape> {
    match p {
        PatternShape::Vertex(t) => Some(t),
        PatternShape::Concat(parts) => parts.first().and_then(first_vertex),
        PatternShape::Group(_, body, _) => first_vertex(body),
        PatternShape::Edge(..) | PatternShape::Union(_) | PatternShape::Alternation(_) | PatternShape::Quantified(..) => None,
    }
}

/// The elements of element table `t` path pattern `path` may start at:
/// of its label index, if it has one, those the reader sees; else every
/// row of a scan.
///
/// # Errors
///
/// Any error reading the table.
pub(super) fn starts(store: &Store, path: &PathShape, t: usize) -> Result<Vec<ElementId>> {
    let table = u32::try_from(t).unwrap_or(u32::MAX);
    let expr = first_vertex(&path.pattern).and_then(|v| v.labels.as_ref());
    let Some(rows) = store.labels().candidates(t, expr) else {
        return Ok(store.rows(t)?.into_iter().map(|(id, _)| ElementId::new(table, id)).collect());
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if store.sees(t, row)? {
            out.push(ElementId::new(table, row));
        }
    }
    Ok(out)
}
