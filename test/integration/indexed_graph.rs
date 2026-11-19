//! \[DESIGN\] `IndexedGraph`, the graph view over element tables whose
//! edges an adjacency keeps or a fallback finds: the hops at a vertex in
//! each orientation, the edges a reader does not see and the vertices it
//! cannot find left out, and what it reads of its rows.

use std::sync::Arc;

use turborel::error::{Code, Result, SqlError};
use turborel::graph::adjacency::{Adjacency, Direction, Step};
use turborel::graph::view::{
    EdgeTable, ElementId, ElementRows, Endpoints, Fallback, GraphView, Hop, IndexedGraph, LabelId, Orientation, TableId, Way,
};
use turborel::index::Key;
use turborel::storage::RowId;
use turborel::value::Value;

/// The vertex table of people: rows 1, 2 and 3, keys A, B and C.
const PEOPLE: TableId = 0;
/// The vertex table of cities: row 10, key X.
const CITIES: TableId = 1;
/// The directed edge table KNOWS of people, its adjacency kept.
const KNOWS: TableId = 2;
/// The undirected edge table LIVES of people and cities, found by the
/// fallback.
const LIVES: TableId = 3;
/// An edge of KNOWS the reader does not see.
const HIDDEN: RowId = 101;

/// The key of text `k`.
fn key(k: &str) -> Key {
    Key(vec![Value::Char(k.to_owned())])
}

/// The rows of the two vertex tables and their edges.
struct Rows;

impl ElementRows for Rows {
    fn key(&self, vertex: ElementId) -> Result<Key> {
        match (vertex.table, vertex.row) {
            (PEOPLE, 1) => Ok(key("A")),
            (PEOPLE, 2) => Ok(key("B")),
            (PEOPLE, 3) => Ok(key("C")),
            (CITIES, 10) => Ok(key("X")),
            _ => Err(SqlError::new(Code::Api, "[DESIGN] no such vertex")),
        }
    }

    fn vertex(&self, table: TableId, k: &Key) -> Result<Option<RowId>> {
        let all = [(PEOPLE, "A", 1), (PEOPLE, "B", 2), (PEOPLE, "C", 3), (CITIES, "X", 10)];
        Ok(all.iter().find(|(t, name, _)| *t == table && key(name) == *k).map(|(_, _, row)| *row))
    }

    fn visible(&self, edge: ElementId) -> Result<bool> {
        Ok(edge.row != HIDDEN)
    }

    fn labels(&self, element: ElementId, out: &mut Vec<LabelId>) -> Result<()> {
        out.push(element.table);
        Ok(())
    }

    fn endpoints(&self, edge: ElementId) -> Result<Endpoints> {
        Ok(Endpoints { source: ElementId::new(PEOPLE, 1), destination: ElementId::new(PEOPLE, 2), directed: edge.table == KNOWS })
    }

    fn cost(&self, edge: ElementId) -> Result<f64> {
        Ok(if edge.row == 100 { 2.5 } else { 1.0 })
    }
}

/// Pushes onto `out` the steps of `held`, each a direction, the key of a
/// vertex, an edge and the key at its other end, of `vertex` in
/// `direction`.
fn listed(held: &[(Direction, &str, RowId, &str)], (vertex, direction): (&Key, Direction), out: &mut Vec<Step>) {
    for (d, from, edge, to) in held {
        if *d == direction && key(from) == *vertex {
            out.push(Step { edge: *edge, vertex: key(to) });
        }
    }
}

/// KNOWS's adjacency: A knows B (edge 100) and C (edge 101, hidden).
struct Knows;

impl Adjacency for Knows {
    fn steps(&self, vertex: &Key, direction: Direction, out: &mut Vec<Step>) -> Result<()> {
        let held = [
            (Direction::Out, "A", 100, "B"),
            (Direction::Out, "A", HIDDEN, "C"),
            (Direction::In, "B", 100, "A"),
            (Direction::In, "C", HIDDEN, "A"),
        ];
        listed(&held, (vertex, direction), out);
        Ok(())
    }

    fn degree(&self, _vertex: &Key, _direction: Direction) -> Result<u64> {
        Ok(1)
    }

    fn edges(&self) -> u64 {
        2
    }
}

/// The graph of KNOWS and LIVES: A lives in X (edge 200), and edge 201
/// joins A to a city no row has.
fn graph() -> IndexedGraph {
    let edges = vec![
        EdgeTable { table: KNOWS, source: PEOPLE, destination: PEOPLE, directed: true, adjacency: Some(Arc::new(Knows)) },
        EdgeTable { table: LIVES, source: PEOPLE, destination: CITIES, directed: false, adjacency: None },
    ];
    let fallback: Fallback = Arc::new(|table: &EdgeTable, k: &Key, direction: Direction, out: &mut Vec<Step>| {
        assert_eq!(table.table, LIVES, "only LIVES lacks an adjacency");
        let held = [(Direction::Out, "A", 200, "X"), (Direction::Out, "A", 201, "NOWHERE"), (Direction::In, "X", 200, "A")];
        listed(&held, (k, direction), out);
        Ok(())
    });
    IndexedGraph::new(edges, Arc::new(Rows), fallback)
}

/// The hops of `vertex` in `orientation`.
fn hops(g: &IndexedGraph, vertex: ElementId, orientation: Orientation) -> Vec<Hop> {
    let mut out = Vec::new();
    g.hops(vertex, orientation, &mut out).unwrap();
    out
}

/// Verifies: [DESIGN] an `IndexedGraph` follows the edges at a vertex
/// through its edge tables' adjacencies, or its fallback for one with
/// none, in each way the orientation allows: a directed edge right from
/// its source and left from its destination, an undirected one from
/// either end; it leaves out an edge the reader does not see and one
/// whose other vertex no row has.
#[test]
fn hops_follow_adjacencies_and_the_fallback() {
    let g = graph();
    let (a, b, x) = (ElementId::new(PEOPLE, 1), ElementId::new(PEOPLE, 2), ElementId::new(CITIES, 10));
    let knows = Hop { edge: ElementId::new(KNOWS, 100), vertex: b, way: Way::Right };
    let lives = Hop { edge: ElementId::new(LIVES, 200), vertex: x, way: Way::Undirected };
    assert_eq!(hops(&g, a, Orientation::ANY), [knows, lives]);
    assert_eq!(hops(&g, a, Orientation::RIGHT), [knows]);
    assert_eq!(hops(&g, a, Orientation::UNDIRECTED), [lives]);
    assert_eq!(hops(&g, a, Orientation::LEFT), []);
    assert_eq!(hops(&g, b, Orientation::LEFT), [Hop { edge: ElementId::new(KNOWS, 100), vertex: a, way: Way::Left }]);
    assert_eq!(hops(&g, ElementId::new(PEOPLE, 3), Orientation::ANY), [], "the one edge at C is hidden");
    assert_eq!(hops(&g, x, Orientation::LEFT_OR_UNDIRECTED), [Hop { edge: ElementId::new(LIVES, 200), vertex: a, way: Way::Undirected }]);
    let mut frontier = Vec::new();
    g.expand(&[(a, Orientation::RIGHT), (x, Orientation::ANY)], &mut frontier).unwrap();
    assert_eq!(frontier.len(), 2);
    assert!(g.size().is_none());
    let mut out = Vec::new();
    assert_eq!(g.hops(ElementId::new(PEOPLE, 9), Orientation::ANY, &mut out).unwrap_err().code, Code::Api);
}

/// Verifies: [DESIGN] an `IndexedGraph` reads an element's labels, an
/// edge's endpoints and its cost from its rows.
#[test]
fn labels_endpoints_and_costs_are_its_rows() {
    let g = graph();
    let edge = ElementId::new(KNOWS, 100);
    let mut labels = Vec::new();
    g.labels(ElementId::new(CITIES, 10), &mut labels).unwrap();
    assert_eq!(labels, [CITIES]);
    let ends = g.endpoints(edge).unwrap();
    assert_eq!((ends.source, ends.destination, ends.directed), (ElementId::new(PEOPLE, 1), ElementId::new(PEOPLE, 2), true));
    assert!(!g.endpoints(ElementId::new(LIVES, 200)).unwrap().directed);
    assert!((g.cost(edge).unwrap() - 2.5).abs() < f64::EPSILON);
}

/// Verifies: [DESIGN] an element's identity reads as its table and row,
/// and each orientation as its edge pattern's arrow [GS §6.3].
#[test]
fn identities_and_orientations_display() {
    assert_eq!(ElementId::new(KNOWS, 100).to_string(), "2:100");
    let arrows = [
        (Orientation::LEFT, "<-"),
        (Orientation::UNDIRECTED, "~"),
        (Orientation::RIGHT, "->"),
        (Orientation::LEFT_OR_UNDIRECTED, "<~"),
        (Orientation::UNDIRECTED_OR_RIGHT, "~>"),
        (Orientation::LEFT_OR_RIGHT, "<->"),
        (Orientation::ANY, "-"),
        (Orientation::NONE, "none"),
    ];
    for (o, arrow) in arrows {
        assert_eq!(o.to_string(), arrow);
    }
    assert_eq!(Orientation::LEFT.reversed(), Orientation::RIGHT);
    assert_eq!(Orientation::LEFT.union(Orientation::UNDIRECTED), Orientation::LEFT_OR_UNDIRECTED);
}
