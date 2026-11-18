//! \[DESIGN\] A vectorized Expand: the hops of a whole frontier of a
//! GRAPH_TABLE's search found at once. The vertices' rows are read in the
//! order of their ids; of each edge table, the lists of every vertex at
//! one of its ends are asked of its adjacency index together
//! (`Adjacency::steps_many`), and the edge rows they name are read once
//! each, in the order of their ids, then checked as one vertex's are
//! (RULES.md D34). A vertex's hops are those it would have alone, so a
//! search's results do not change.

use std::collections::HashMap;

use super::{Store, key_text, table_id, values_of};
use crate::error::Result;
use crate::graph::adjacency::{Direction, Step};
use crate::graph::view::{ElementId, Hop, Orientation, Way};
use crate::index::Key;
use crate::logical::EndShape;
use crate::storage::{Row, RowId};

/// The edge rows read of a frontier's lists, by edge table and id: none of
/// a row the reader does not see.
type EdgeRows = HashMap<(usize, RowId), Option<Row>>;

/// A frontier being expanded: what is asked, and its vertices' rows.
struct Frontier<'a> {
    asked: &'a [(ElementId, Orientation)],
    rows: Vec<Option<Row>>,
}

/// One end of an edge table, followed one way: the table, whether from its
/// sources, its end there and its other end, and how its hops are made.
#[derive(Clone, Copy)]
struct Side<'a> {
    table: usize,
    forward: bool,
    at: &'a EndShape,
    other: &'a EndShape,
    /// The way its hops are followed.
    way: Way,
    /// Whether an edge from a vertex to itself was followed already, from
    /// its source.
    looped: bool,
}

impl Store {
    /// `GraphView::expand`: of each of `asked`, its vertex's hops in its
    /// orientation, pushed onto `out` in the order asked.
    ///
    /// # Errors
    ///
    /// Any error reading a row or an adjacency index.
    pub(super) fn expand_all(&self, asked: &[(ElementId, Orientation)], out: &mut Vec<Vec<Hop>>) -> Result<()> {
        let frontier = Frontier { asked, rows: self.vertex_rows(asked)? };
        let mut found: Vec<Vec<Hop>> = vec![Vec::new(); asked.len()];
        let mut edges = EdgeRows::new();
        for (t, source) in self.graph.tables.iter().enumerate() {
            let Some(ends) = &source.ends else { continue };
            let (out_way, in_way) = if ends.directed { (Way::Right, Way::Left) } else { (Way::Undirected, Way::Undirected) };
            for (forward, way) in [(true, out_way), (false, in_way)] {
                let (at, other) = if forward { (&ends.source, &ends.destination) } else { (&ends.destination, &ends.source) };
                // An undirected self-loop was followed from its source.
                let looped = !forward && !ends.directed && ends.source.table == ends.destination.table;
                let side = Side { table: t, forward, at, other, way, looped };
                self.follow(side, &frontier, (&mut found, &mut edges))?;
            }
        }
        out.extend(found);
        Ok(())
    }

    /// Pushes onto `found` the hops of `side` of each vertex of the frontier
    /// it follows from.
    fn follow(&self, side: Side<'_>, frontier: &Frontier<'_>, (found, edges): (&mut [Vec<Hop>], &mut EdgeRows)) -> Result<()> {
        let places: Vec<usize> = (0..frontier.asked.len()).filter(|&p| frontier.follows(p, (table_id(side.at.table), side.way))).collect();
        if places.is_empty() {
            return Ok(());
        }
        let lists = self.lists_of(side, (frontier, &places), edges)?;
        for (place, list) in places.into_iter().zip(lists) {
            let (Some(&(vertex, _)), Some(hops)) = (frontier.asked.get(place), found.get_mut(place)) else { continue };
            let kept = list.into_iter().filter(|&(_, o)| !(side.looped && o == vertex.row));
            hops.extend(kept.map(|(edge, o)| Hop {
                edge: ElementId::new(table_id(side.table), edge),
                vertex: ElementId::new(table_id(side.other.table), o),
                way: side.way,
            }));
        }
        Ok(())
    }

    /// The rows of the vertices of `asked`, read in the order of their ids.
    fn vertex_rows(&self, asked: &[(ElementId, Orientation)]) -> Result<Vec<Option<Row>>> {
        let mut order: Vec<usize> = (0..asked.len()).collect();
        order.sort_unstable_by_key(|&p| asked.get(p).map(|a| a.0));
        let mut rows = vec![None; asked.len()];
        for p in order {
            if let (Some(&(vertex, _)), Some(slot)) = (asked.get(p), rows.get_mut(p)) {
                *slot = self.row(vertex)?;
            }
        }
        Ok(rows)
    }

    /// Of each of `places` of the frontier, the edges of `side` at its
    /// vertex: each edge and the vertex at its other end.
    fn lists_of(
        &self,
        side: Side<'_>,
        (frontier, places): (&Frontier<'_>, &[usize]),
        edges: &mut EdgeRows,
    ) -> Result<Vec<Vec<(RowId, RowId)>>> {
        let Some(adjacency) = self.adjacency.get(side.table).and_then(Option::as_ref) else {
            let listed = self.listed(side.table, side.forward)?;
            let vertex = |p: usize| frontier.asked.get(p).map_or(0, |a| a.0.row);
            return Ok(places.iter().map(|&p| listed.get(&vertex(p)).cloned().unwrap_or_default()).collect());
        };
        let keys: Vec<Key> =
            places.iter().map(|&p| Key(frontier.row(p).map(|r| values_of(r, &side.at.references)).unwrap_or_default())).collect();
        let mut lists: Vec<Vec<Step>> = Vec::with_capacity(keys.len());
        adjacency.steps_many(&keys, if side.forward { Direction::Out } else { Direction::In }, &mut lists)?;
        self.read_edges(side.table, &lists, edges)?;
        let mut found = Vec::with_capacity(lists.len());
        for (key, steps) in keys.iter().zip(lists) {
            let mut of = Vec::new();
            for step in steps {
                // An entry may be of a version the reader does not see, or
                // whose ends have changed since (RULES.md D34).
                let Some(Some(edge_row)) = edges.get(&(side.table, step.edge)) else { continue };
                if key_text(&values_of(edge_row, &side.at.columns)) != key_text(&key.0) {
                    continue;
                }
                of.extend(self.ends_of(side.other, edge_row)?.into_iter().map(|o| (step.edge, o)));
            }
            found.push(of);
        }
        Ok(found)
    }

    /// Reads into `edges` each row of edge table `table` that `lists` name
    /// and it lacks, once each, in the order of their ids.
    fn read_edges(&self, table: usize, lists: &[Vec<Step>], edges: &mut EdgeRows) -> Result<()> {
        let mut wanted: Vec<RowId> = lists.iter().flatten().map(|s| s.edge).filter(|&e| !edges.contains_key(&(table, e))).collect();
        wanted.sort_unstable();
        wanted.dedup();
        for edge in wanted {
            edges.insert((table, edge), self.row(ElementId::new(table_id(table), edge))?);
        }
        Ok(())
    }
}

impl Frontier<'_> {
    /// The row of the vertex at place `p`, if the reader sees it.
    fn row(&self, p: usize) -> Option<&Row> {
        self.rows.get(p).and_then(Option::as_ref)
    }

    /// Whether the vertex at place `p` is of vertex table `table`, seen,
    /// and asked of edges followed `way`.
    fn follows(&self, p: usize, (table, way): (u32, Way)) -> bool {
        self.asked.get(p).is_some_and(|&(v, o)| v.table == table && o.allows(way)) && self.row(p).is_some()
    }
}
