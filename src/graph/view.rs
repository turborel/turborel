//! \[DESIGN\] The graph a path search walks, whatever holds it: elements
//! named by their element table and row, their labels, an edge's
//! endpoints and cost, and the edges at a vertex in the orientations an
//! edge pattern allows [GS §3, §6.3].
//!
//! A graph is a multigraph whose edges are directed or undirected
//! [GS §3] [P1 §2]: each edge followed from a vertex is a `Hop`, saying
//! which way it was followed, so the seven orientations of an edge
//! pattern [GS §6.3] are sets of ways.
//!
//! `IndexedGraph` is a view over element tables whose edges an
//! `Adjacency` keeps, with a closure for an edge table it does not: the
//! search reads every graph through the same trait.

use std::fmt;
use std::sync::Arc;

use super::adjacency::{Adjacency, Direction, Step};
use crate::error::{Code, Result, SqlError};
use crate::index::Key;
use crate::storage::RowId;

/// The number of an element table within its graph.
pub type TableId = u32;

/// The number of a label within its graph.
pub type LabelId = u32;

/// \[GS §5 R5.1\] An element's identity: its element table and its row,
/// stable across updates. Its order is the order DETERMINISTIC_PATHS
/// breaks ties by [GS §7.2 R7.2].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementId {
    /// Its element table.
    pub table: TableId,
    /// Its row in that table.
    pub row: RowId,
}

impl ElementId {
    /// The element of row `row` of element table `table`.
    pub fn new(table: TableId, row: RowId) -> ElementId {
        ElementId { table, row }
    }
}

impl fmt::Display for ElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.table, self.row)
    }
}

/// The way an edge is followed [GS §6.3] [P1 Fig. 5].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Way {
    /// A directed edge from its source to its destination, pointing right.
    Right,
    /// A directed edge from its destination to its source, pointing left.
    Left,
    /// An undirected edge, from either end.
    Undirected,
}

impl Way {
    /// Its bit in an `Orientation`.
    fn bit(self) -> u8 {
        match self {
            Way::Right => RIGHT_BIT,
            Way::Left => LEFT_BIT,
            Way::Undirected => UNDIRECTED_BIT,
        }
    }
}

/// The bit of `Way::Right` in an orientation.
const RIGHT_BIT: u8 = 1;
/// The bit of `Way::Left`.
const LEFT_BIT: u8 = 2;
/// The bit of `Way::Undirected`.
const UNDIRECTED_BIT: u8 = 4;

/// \[GS §6.3\] The ways an edge pattern matches, one of the seven
/// orientations of [P1 Fig. 5], or a union of them for a search's needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Orientation(u8);

impl Orientation {
    /// `<-[ ]-`: directed, pointing left.
    pub const LEFT: Orientation = Orientation(LEFT_BIT);
    /// `~[ ]~`: undirected.
    pub const UNDIRECTED: Orientation = Orientation(UNDIRECTED_BIT);
    /// `-[ ]->`: directed, pointing right.
    pub const RIGHT: Orientation = Orientation(RIGHT_BIT);
    /// `<~[ ]~`: left or undirected.
    pub const LEFT_OR_UNDIRECTED: Orientation = Orientation(LEFT_BIT | UNDIRECTED_BIT);
    /// `~[ ]~>`: undirected or right.
    pub const UNDIRECTED_OR_RIGHT: Orientation = Orientation(UNDIRECTED_BIT | RIGHT_BIT);
    /// `<-[ ]->`: left or right.
    pub const LEFT_OR_RIGHT: Orientation = Orientation(LEFT_BIT | RIGHT_BIT);
    /// `-[ ]-`: any orientation.
    pub const ANY: Orientation = Orientation(LEFT_BIT | RIGHT_BIT | UNDIRECTED_BIT);
    /// No way at all: the union of none.
    pub const NONE: Orientation = Orientation(0);

    /// Whether it matches an edge followed `way`.
    pub fn allows(self, way: Way) -> bool {
        self.0 & way.bit() != 0
    }

    /// The ways either matches.
    #[must_use]
    pub fn union(self, other: Orientation) -> Orientation {
        Orientation(self.0 | other.0)
    }

    /// The orientation matching the edges this one matches, followed
    /// from their other end: left and right swapped.
    #[must_use]
    pub fn reversed(self) -> Orientation {
        let swapped = (self.0 & UNDIRECTED_BIT) | if self.allows(Way::Left) { RIGHT_BIT } else { 0 };
        Orientation(swapped | if self.allows(Way::Right) { LEFT_BIT } else { 0 })
    }
}

impl fmt::Display for Orientation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (left, right, undirected) = (self.allows(Way::Left), self.allows(Way::Right), self.allows(Way::Undirected));
        let arrow = match (left, undirected, right) {
            (true, false, false) => "<-",
            (false, true, false) => "~",
            (false, false, true) => "->",
            (true, true, false) => "<~",
            (false, true, true) => "~>",
            (true, false, true) => "<->",
            (true, true, true) => "-",
            (false, false, false) => "none",
        };
        f.write_str(arrow)
    }
}

/// One edge followed from a vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hop {
    /// The edge.
    pub edge: ElementId,
    /// The vertex at its other end.
    pub vertex: ElementId,
    /// Which way it was followed.
    pub way: Way,
}

/// An edge's endpoints [GS §3]: an ordered pair if it is directed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// Its source, or one end of an undirected edge.
    pub source: ElementId,
    /// Its destination, or the other end.
    pub destination: ElementId,
    /// Whether it is directed.
    pub directed: bool,
}

/// \[DESIGN\] A property graph as a path search reads it [GS §3]. It may
/// be shared between the threads of a parallel search, so it is `Sync`.
pub trait GraphView: Send + Sync {
    /// Pushes onto `out` the labels of `element` [GS §5 R5.4].
    ///
    /// # Errors
    ///
    /// Any error reading the element.
    fn labels(&self, element: ElementId, out: &mut Vec<LabelId>) -> Result<()>;

    /// Pushes onto `out` each edge at `vertex` followed in a way
    /// `orientation` allows, once per way it may be followed.
    ///
    /// # Errors
    ///
    /// Any error reading the edges.
    fn hops(&self, vertex: ElementId, orientation: Orientation, out: &mut Vec<Hop>) -> Result<()>;

    /// The endpoints of `edge` [GS §5 R5.3].
    ///
    /// # Errors
    ///
    /// Any error reading the edge.
    fn endpoints(&self, edge: ElementId) -> Result<Endpoints>;

    /// The cost of following `edge`, for CHEAPEST [GS §7.2], where the
    /// pattern gives no COST of its own.
    ///
    /// # Errors
    ///
    /// Any error reading the edge.
    fn cost(&self, edge: ElementId) -> Result<f64>;
}

/// \[DESIGN\] An edge table of an `IndexedGraph`: its endpoints' vertex
/// tables, whether its edges are directed, and the adjacency an index
/// keeps of it, if one does.
#[derive(Clone)]
pub struct EdgeTable {
    /// Its number in the graph.
    pub table: TableId,
    /// The vertex table of its sources.
    pub source: TableId,
    /// The vertex table of its destinations.
    pub destination: TableId,
    /// Whether its edges are directed.
    pub directed: bool,
    /// Its adjacency, if an index keeps one.
    pub adjacency: Option<Arc<dyn Adjacency>>,
}

/// \[DESIGN\] What an `IndexedGraph` reads of its element tables' rows,
/// which the executor provides: the key of a vertex, the row of a key,
/// whether an edge row is visible to the reader, an element's labels, an
/// edge's endpoints and cost.
pub trait ElementRows: Send + Sync {
    /// The key of vertex `vertex`, as its edges' adjacency names it.
    ///
    /// # Errors
    ///
    /// Any error reading the row.
    fn key(&self, vertex: ElementId) -> Result<Key>;
    /// The row of vertex table `table` whose key is `key`, if a visible one is.
    ///
    /// # Errors
    ///
    /// Any error reading the table.
    fn vertex(&self, table: TableId, key: &Key) -> Result<Option<RowId>>;
    /// Whether the reader sees edge `edge`'s row: an adjacency's entries
    /// may be of versions it does not (RULES.md D34).
    ///
    /// # Errors
    ///
    /// Any error reading the row.
    fn visible(&self, edge: ElementId) -> Result<bool>;
    /// As `GraphView::labels`.
    ///
    /// # Errors
    ///
    /// As `GraphView::labels`.
    fn labels(&self, element: ElementId, out: &mut Vec<LabelId>) -> Result<()>;
    /// As `GraphView::endpoints`.
    ///
    /// # Errors
    ///
    /// As `GraphView::endpoints`.
    fn endpoints(&self, edge: ElementId) -> Result<Endpoints>;
    /// As `GraphView::cost`.
    ///
    /// # Errors
    ///
    /// As `GraphView::cost`.
    fn cost(&self, edge: ElementId) -> Result<f64>;
}

/// \[DESIGN\] The steps of an edge table no adjacency keeps, found by the
/// caller some other way, as a scan of the edge table would: the edges of
/// `table` at the vertex of key `vertex`, in `direction`.
pub type Fallback = Arc<dyn Fn(&EdgeTable, &Key, Direction, &mut Vec<Step>) -> Result<()> + Send + Sync>;

/// \[DESIGN\] A graph over element tables whose edges are found through
/// adjacencies, or through the fallback where an edge table has none.
pub struct IndexedGraph {
    edges: Vec<EdgeTable>,
    rows: Arc<dyn ElementRows>,
    fallback: Fallback,
}

impl IndexedGraph {
    /// The graph of edge tables `edges`, read through `rows`, with
    /// `fallback` for an edge table without an adjacency.
    pub fn new(edges: Vec<EdgeTable>, rows: Arc<dyn ElementRows>, fallback: Fallback) -> IndexedGraph {
        IndexedGraph { edges, rows, fallback }
    }

    /// The steps of `table` at `key` in `direction`.
    fn steps(&self, table: &EdgeTable, key: &Key, direction: Direction, out: &mut Vec<Step>) -> Result<()> {
        match &table.adjacency {
            Some(adjacency) => adjacency.steps(key, direction, out),
            None => (self.fallback)(table, key, direction, out),
        }
    }

    /// Pushes the hops of `table` at `vertex`, of key `key`, followed
    /// `way` in `direction`, whose other ends are of vertex table `other`.
    fn push(&self, table: &EdgeTable, key: &Key, (way, direction, other): (Way, Direction, TableId), out: &mut Vec<Hop>) -> Result<()> {
        let mut steps = Vec::new();
        self.steps(table, key, direction, &mut steps)?;
        for step in steps {
            let edge = ElementId::new(table.table, step.edge);
            // An entry may be of a version the reader cannot see, and an
            // edge's other end may be gone from a graph whose keys are
            // trusted, not enforced: either drops the edge [GS §4.1 R4.3].
            if !self.rows.visible(edge)? {
                continue;
            }
            if let Some(row) = self.rows.vertex(other, &step.vertex)? {
                out.push(Hop { edge, vertex: ElementId::new(other, row), way });
            }
        }
        Ok(())
    }
}

impl GraphView for IndexedGraph {
    fn labels(&self, element: ElementId, out: &mut Vec<LabelId>) -> Result<()> {
        self.rows.labels(element, out)
    }

    fn hops(&self, vertex: ElementId, orientation: Orientation, out: &mut Vec<Hop>) -> Result<()> {
        let key = self.rows.key(vertex)?;
        for table in &self.edges {
            let (out_way, in_way) = if table.directed { (Way::Right, Way::Left) } else { (Way::Undirected, Way::Undirected) };
            if table.source == vertex.table && orientation.allows(out_way) {
                self.push(table, &key, (out_way, Direction::Out, table.destination), out)?;
            }
            if table.destination == vertex.table && orientation.allows(in_way) {
                self.push(table, &key, (in_way, Direction::In, table.source), out)?;
            }
        }
        Ok(())
    }

    fn endpoints(&self, edge: ElementId) -> Result<Endpoints> {
        self.rows.endpoints(edge)
    }

    fn cost(&self, edge: ElementId) -> Result<f64> {
        self.rows.cost(edge)
    }
}

/// The error of an element a view does not hold.
pub(super) fn no_element(element: ElementId) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] the graph has no element {element}"))
}
