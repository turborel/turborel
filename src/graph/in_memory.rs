//! \[DESIGN\] A property graph held in memory [GS §3]: vertices and
//! edges with labels, integer and text properties and a cost. Path
//! searches are tested and measured on it, and a graph built from a
//! query's rows may be held so.

use std::collections::HashMap;

use super::view::{ElementId, Endpoints, GraphSize, GraphView, Hop, LabelId, Orientation, TableId, Way, no_element};
use crate::error::Result;
use crate::storage::RowId;

/// A property's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Property {
    /// An integer.
    Int(i64),
    /// A string.
    Text(String),
}

/// One element's labels and properties.
#[derive(Clone, Debug, Default)]
struct Held {
    labels: Vec<LabelId>,
    properties: Vec<(String, Property)>,
}

/// One edge: its endpoints, labels, properties and cost.
#[derive(Clone, Debug)]
struct HeldEdge {
    ends: Endpoints,
    held: Held,
    cost: f64,
}

/// A graph held in memory, its elements numbered within their tables in
/// the order they are added.
#[derive(Debug, Default)]
pub struct InMemoryGraph {
    vertices: HashMap<ElementId, Held>,
    edges: HashMap<ElementId, HeldEdge>,
    /// The edges at each vertex, in the order added.
    incident: HashMap<ElementId, Vec<(ElementId, Endpoints)>>,
    /// The next row of each table.
    next: HashMap<TableId, RowId>,
}

impl InMemoryGraph {
    /// An empty graph.
    pub fn new() -> InMemoryGraph {
        InMemoryGraph::default()
    }

    /// The next row of `table`.
    fn row(&mut self, table: TableId) -> ElementId {
        let next = self.next.entry(table).or_insert(0);
        let row = *next;
        *next = next.saturating_add(1);
        ElementId::new(table, row)
    }

    /// Adds a vertex to `table` with `labels`.
    pub fn vertex(&mut self, table: TableId, labels: &[LabelId]) -> ElementId {
        let id = self.row(table);
        self.vertices.insert(id, Held { labels: labels.to_vec(), properties: Vec::new() });
        self.incident.insert(id, Vec::new());
        id
    }

    /// Adds an edge to `table` with `labels`, from `source` to
    /// `destination` if `directed`, else between them, of cost 1.
    pub fn edge(&mut self, table: TableId, (source, destination): (ElementId, ElementId), directed: bool, labels: &[LabelId]) -> ElementId {
        let id = self.row(table);
        let ends = Endpoints { source, destination, directed };
        self.edges.insert(id, HeldEdge { ends, held: Held { labels: labels.to_vec(), properties: Vec::new() }, cost: 1.0 });
        self.incident.entry(source).or_default().push((id, ends));
        if destination != source {
            self.incident.entry(destination).or_default().push((id, ends));
        }
        id
    }

    /// Sets property `name` of `element` to `value`.
    pub fn set(&mut self, element: ElementId, name: &str, value: Property) {
        let held = match self.vertices.get_mut(&element) {
            Some(held) => Some(held),
            None => self.edges.get_mut(&element).map(|edge| &mut edge.held),
        };
        if let Some(held) = held {
            held.properties.retain(|(n, _)| n != name);
            held.properties.push((name.to_owned(), value));
        }
    }

    /// Sets the cost of edge `edge`.
    pub fn set_cost(&mut self, edge: ElementId, cost: f64) {
        if let Some(held) = self.edges.get_mut(&edge) {
            held.cost = cost;
        }
    }

    /// Property `name` of `element`, if it has it.
    pub fn property(&self, element: ElementId, name: &str) -> Option<&Property> {
        let held = self.vertices.get(&element).or_else(|| self.edges.get(&element).map(|edge| &edge.held))?;
        held.properties.iter().find(|(n, _)| n == name).map(|(_, value)| value)
    }

    /// Integer property `name` of `element`, if it has it.
    pub fn int(&self, element: ElementId, name: &str) -> Option<i64> {
        match self.property(element, name) {
            Some(Property::Int(n)) => Some(*n),
            Some(Property::Text(_)) | None => None,
        }
    }

    /// Text property `name` of `element`, if it has it.
    pub fn text(&self, element: ElementId, name: &str) -> Option<&str> {
        match self.property(element, name) {
            Some(Property::Text(s)) => Some(s),
            Some(Property::Int(_)) | None => None,
        }
    }

    /// Its vertices, in element order.
    pub fn vertices(&self) -> Vec<ElementId> {
        let mut all: Vec<ElementId> = self.vertices.keys().copied().collect();
        all.sort_unstable();
        all
    }

    /// Its edges, in element order.
    pub fn edges(&self) -> Vec<ElementId> {
        let mut all: Vec<ElementId> = self.edges.keys().copied().collect();
        all.sort_unstable();
        all
    }

    /// The ways `edge` of `ends` is followed from `vertex`, with the other end.
    fn ways(ends: &Endpoints, vertex: ElementId) -> [Option<(Way, ElementId)>; 2] {
        let from_source = (ends.source == vertex).then_some(ends.destination);
        let from_destination = (ends.destination == vertex).then_some(ends.source);
        if ends.directed {
            [from_source.map(|v| (Way::Right, v)), from_destination.map(|v| (Way::Left, v))]
        } else {
            // A self-loop is followed once from its one end.
            [from_source.map(|v| (Way::Undirected, v)), from_destination.filter(|_| from_source.is_none()).map(|v| (Way::Undirected, v))]
        }
    }
}

impl GraphView for InMemoryGraph {
    fn labels(&self, element: ElementId, out: &mut Vec<LabelId>) -> Result<()> {
        let held = self.vertices.get(&element).or_else(|| self.edges.get(&element).map(|edge| &edge.held));
        out.extend_from_slice(&held.ok_or_else(|| no_element(element))?.labels);
        Ok(())
    }

    fn hops(&self, vertex: ElementId, orientation: Orientation, out: &mut Vec<Hop>) -> Result<()> {
        for &(edge, ends) in self.incident.get(&vertex).ok_or_else(|| no_element(vertex))? {
            for (way, other) in InMemoryGraph::ways(&ends, vertex).into_iter().flatten() {
                if orientation.allows(way) {
                    out.push(Hop { edge, vertex: other, way });
                }
            }
        }
        Ok(())
    }

    fn endpoints(&self, edge: ElementId) -> Result<Endpoints> {
        Ok(self.edges.get(&edge).ok_or_else(|| no_element(edge))?.ends)
    }

    fn cost(&self, edge: ElementId) -> Result<f64> {
        Ok(self.edges.get(&edge).ok_or_else(|| no_element(edge))?.cost)
    }

    fn size(&self) -> Option<GraphSize> {
        Some(GraphSize { vertices: u64::try_from(self.vertices.len()).ok()?, edges: u64::try_from(self.edges.len()).ok()? })
    }
}
