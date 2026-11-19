//! \[EXTENSION\] The graph a GRAPH_TABLE's search reads (`GraphView`):
//! its element tables' rows as the statement's transaction sees them.
//!
//! A base table's rows are read where they are stored, each by its id,
//! and the transaction is told it read the whole table, as a scan tells
//! it. A view's, or a table's read FOR SYSTEM_TIME, are the rows of their
//! plan, held for the statement and numbered as read.
//!
//! An edge is followed through the adjacency index of its table that
//! keeps its ends' columns, if one does, each entry's row read again, as
//! one may be of a version the reader does not see and its ends must
//! still be the vertex's (RULES.md D34). Without one, the edges of the
//! table are listed by end the first time one is followed. An end is
//! found by the columns it references, every vertex row of them, so a
//! TRUSTED graph whose keys repeat binds each; an edge whose end is no
//! row is no edge [GS §4.1 R4.3]. An undirected edge from a vertex to
//! itself is followed once. A frontier's edges are found at once
//! (`store/expand.rs`), and one vertex's as a frontier of one. Its size,
//! for the search's choice of algorithm and workers, is its tables' row
//! counts.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use super::labels::GraphLabels;
use crate::db::State;
use crate::error::Result;
use crate::graph::adjacency::Adjacency;
use crate::graph::view::{ElementId, Endpoints, GraphSize, GraphView, Hop, LabelId, Orientation};
use crate::index::Probe;
use crate::logical::{EndShape, GraphMatch};
use crate::storage::{Condition, Row, RowId, TableStorage};
use crate::txn::TxId;
use crate::value::Value;

mod expand;

/// The rows of an element table.
pub(super) enum TableRows {
    /// Where they are stored.
    Stored(Arc<dyn TableStorage>),
    /// Held, numbered as read.
    Held(Vec<Row>),
}

/// The edges of an edge table at each vertex, one way: the edge and the
/// vertex at its other end.
type Listed = HashMap<RowId, Vec<(RowId, RowId)>>;

/// The rows of a vertex table by the text of the columns an end
/// references.
type Lookup = HashMap<String, Vec<RowId>>;

/// \[EXTENSION\] The graph of a GRAPH_TABLE's element tables.
pub(super) struct Store {
    tx: TxId,
    graph: Arc<GraphMatch>,
    tables: Vec<TableRows>,
    adjacency: Vec<Option<Arc<dyn Adjacency>>>,
    lists: Mutex<HashMap<(usize, bool), Arc<Listed>>>,
    lookups: Mutex<Lookups>,
    /// The label bitmaps of its label indexes (RULES.md D115).
    labels: GraphLabels,
}

/// The lookups made of each vertex table and its key's columns.
type Lookups = HashMap<(usize, Vec<usize>), Arc<Lookup>>;

/// The text a key of `values` is found by; none if one is null, which
/// matches no row.
pub(super) fn key_text(values: &[Value]) -> Option<String> {
    if values.iter().any(Value::is_null) {
        return None;
    }
    Some(values.iter().map(|v| format!("{v:?}")).collect::<Vec<_>>().join("\u{1}"))
}

/// The values of `columns` of `row`.
fn values_of(row: &Row, columns: &[usize]) -> Vec<Value> {
    columns.iter().map(|&c| row.get(c).cloned().unwrap_or(Value::Null)).collect()
}

/// The adjacency index `name` keeps, if it is one.
fn adjacency_of(db: &State, name: &str) -> Option<Arc<dyn Adjacency>> {
    Arc::clone(db.index_of(name).ok()?).adjacency()
}

impl Store {
    /// The graph of `graph`'s element tables, `held` the rows of each one
    /// not stored, read by transaction `tx` of `db`.
    ///
    /// # Errors
    ///
    /// Any error opening a table.
    pub(super) fn new(db: &Arc<State>, tx: TxId, graph: Arc<GraphMatch>, mut held: Vec<Option<Vec<Row>>>) -> Result<Store> {
        let mut tables = Vec::new();
        let mut adjacency = Vec::new();
        for (k, source) in graph.tables.iter().enumerate() {
            let rows = if let Some(rows) = held.get_mut(k).and_then(Option::take) {
                TableRows::Held(rows)
            } else {
                let storage = Arc::clone(db.storage_of(&source.table)?);
                storage.read_by_condition(tx, &Condition { columns: Vec::new(), probe: Probe::All })?;
                TableRows::Stored(storage)
            };
            let stored = matches!(rows, TableRows::Stored(_));
            adjacency.push(source.adjacency.as_deref().filter(|_| stored).and_then(|ix| adjacency_of(db, ix)));
            tables.push(rows);
        }
        let labels = GraphLabels::new(db, &graph)?;
        Ok(Store { tx, graph, tables, adjacency, lists: Mutex::new(HashMap::new()), lookups: Mutex::new(HashMap::new()), labels })
    }

    /// The row of `element`, if the reader sees it.
    ///
    /// # Errors
    ///
    /// Any error reading it.
    pub(super) fn row(&self, element: ElementId) -> Result<Option<Row>> {
        let table = usize::try_from(element.table).unwrap_or(usize::MAX);
        match self.tables.get(table) {
            Some(TableRows::Stored(s)) => s.get(self.tx, element.row),
            Some(TableRows::Held(rows)) => Ok(usize::try_from(element.row).ok().and_then(|r| rows.get(r)).cloned()),
            None => Ok(None),
        }
    }

    /// The label bitmaps of its label indexes.
    pub(super) fn labels(&self) -> &GraphLabels {
        &self.labels
    }

    /// Whether the reader sees row `row` of element table `table`, read
    /// without its columns.
    ///
    /// # Errors
    ///
    /// Any error reading it.
    pub(super) fn sees(&self, table: usize, row: RowId) -> Result<bool> {
        match self.tables.get(table) {
            Some(TableRows::Stored(s)) => Ok(s.get_columns(self.tx, row, &[])?.is_some()),
            Some(TableRows::Held(rows)) => Ok(usize::try_from(row).is_ok_and(|r| r < rows.len())),
            None => Ok(false),
        }
    }

    /// Every row of element table `table`.
    ///
    /// # Errors
    ///
    /// Any error reading it.
    pub(super) fn rows(&self, table: usize) -> Result<Vec<(RowId, Row)>> {
        let width = self.graph.tables.get(table).map_or(0, |t| t.width);
        match self.tables.get(table) {
            Some(TableRows::Stored(s)) => Arc::clone(s).scan(self.tx, &(0..width).collect::<Vec<_>>()).collect(),
            Some(TableRows::Held(rows)) => {
                Ok(rows.iter().enumerate().map(|(k, r)| (u64::try_from(k).unwrap_or(u64::MAX), r.clone())).collect())
            }
            None => Ok(Vec::new()),
        }
    }

    /// The rows of vertex table `table` whose `columns` have the text
    /// `key`.
    fn found(&self, (table, columns): (usize, &[usize]), key: &str) -> Result<Vec<RowId>> {
        let made = self.lookups.lock().unwrap_or_else(PoisonError::into_inner).get(&(table, columns.to_vec())).cloned();
        let lookup = if let Some(l) = made {
            l
        } else {
            let mut l = Lookup::new();
            for (id, row) in self.rows(table)? {
                if let Some(k) = key_text(&values_of(&row, columns)) {
                    l.entry(k).or_default().push(id);
                }
            }
            let l = Arc::new(l);
            self.lookups.lock().unwrap_or_else(PoisonError::into_inner).insert((table, columns.to_vec()), Arc::clone(&l));
            l
        };
        Ok(lookup.get(key).cloned().unwrap_or_default())
    }

    /// The vertices at end `end` of edge row `row`.
    fn ends_of(&self, end: &EndShape, row: &Row) -> Result<Vec<RowId>> {
        match key_text(&values_of(row, &end.columns)) {
            Some(k) => self.found((end.table, &end.references), &k),
            None => Ok(Vec::new()),
        }
    }

    /// The edges of edge table `table` listed by their sources (`out`) or
    /// destinations.
    fn listed(&self, table: usize, out: bool) -> Result<Arc<Listed>> {
        if let Some(l) = self.lists.lock().unwrap_or_else(PoisonError::into_inner).get(&(table, out)) {
            return Ok(Arc::clone(l));
        }
        let ends = self.graph.tables.get(table).and_then(|t| t.ends.as_ref());
        let mut listed = Listed::new();
        if let Some(ends) = ends {
            let (from, to) = if out { (&ends.source, &ends.destination) } else { (&ends.destination, &ends.source) };
            for (edge, row) in self.rows(table)? {
                let others = self.ends_of(to, &row)?;
                for at in self.ends_of(from, &row)? {
                    listed.entry(at).or_default().extend(others.iter().map(|&o| (edge, o)));
                }
            }
        }
        let listed = Arc::new(listed);
        self.lists.lock().unwrap_or_else(PoisonError::into_inner).insert((table, out), Arc::clone(&listed));
        Ok(listed)
    }
}

/// The number of element table `t`, as an element's.
fn table_id(t: usize) -> u32 {
    u32::try_from(t).unwrap_or(u32::MAX)
}

impl GraphView for Store {
    fn labels(&self, element: ElementId, out: &mut Vec<LabelId>) -> Result<()> {
        if self.labels.labels_of(element, out) {
            return Ok(());
        }
        let table = self.graph.tables.get(usize::try_from(element.table).unwrap_or(usize::MAX));
        out.extend(table.map(|t| t.labels.as_slice()).unwrap_or_default());
        Ok(())
    }

    fn hops(&self, vertex: ElementId, orientation: Orientation, out: &mut Vec<Hop>) -> Result<()> {
        let mut lists = Vec::with_capacity(1);
        self.expand_all(&[(vertex, orientation)], &mut lists)?;
        out.extend(lists.into_iter().flatten());
        Ok(())
    }

    fn expand(&self, asked: &[(ElementId, Orientation)], out: &mut Vec<Vec<Hop>>) -> Result<()> {
        self.expand_all(asked, out)
    }

    fn size(&self) -> Option<GraphSize> {
        let (mut vertices, mut edges) = (0_u64, 0_u64);
        for (source, rows) in self.graph.tables.iter().zip(&self.tables) {
            let n = u64::try_from(match rows {
                TableRows::Stored(s) => s.row_count(),
                TableRows::Held(rows) => rows.len(),
            })
            .ok()?;
            if source.ends.is_some() {
                edges = edges.saturating_add(n);
            } else {
                vertices = vertices.saturating_add(n);
            }
        }
        Some(GraphSize { vertices, edges })
    }

    fn endpoints(&self, edge: ElementId) -> Result<Endpoints> {
        let source = self.graph.tables.get(usize::try_from(edge.table).unwrap_or(usize::MAX));
        let ends = source.and_then(|s| s.ends.as_ref()).ok_or_else(|| super::broken("an edge of no edge table"))?;
        let row = self.row(edge)?.ok_or_else(|| super::broken("an edge's row is gone"))?;
        let s = self.ends_of(&ends.source, &row)?.first().copied().ok_or_else(|| super::broken("an edge has no source"))?;
        let d = self.ends_of(&ends.destination, &row)?.first().copied().ok_or_else(|| super::broken("an edge has no destination"))?;
        Ok(Endpoints {
            source: ElementId::new(table_id(ends.source.table), s),
            destination: ElementId::new(table_id(ends.destination.table), d),
            directed: ends.directed,
        })
    }

    fn cost(&self, _edge: ElementId) -> Result<f64> {
        Ok(1.0)
    }
}
