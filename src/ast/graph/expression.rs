//! \[EXTENSION\] [P1 §4.7] The predicates and functions a graph pattern's
//! conditions and GRAPH_TABLE's columns add to SQL's: of element
//! identity, orientation, labels and properties, and of paths. The binder
//! refuses each outside a GRAPH_TABLE.

use super::{GraphVariable, LabelExpression, ParenthesizedPath};

/// The kind of graph variable an operand must name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariableKind {
    /// A node's or an edge's.
    Element,
    /// A node's.
    Vertex,
    /// An edge's.
    Edge,
    /// A path's.
    Path,
    /// An element's or a path's.
    ElementOrPath,
}

/// A function of a graph variable, named as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFunctionName {
    /// ELEMENT_ID: an element's identity, or a path's elements'.
    ElementId,
    /// \[EXTENSION\] VERTEX_ID: a node's identity, as JSON.
    VertexId,
    /// \[EXTENSION\] EDGE_ID: an edge's identity, as JSON.
    EdgeId,
    /// \[EXTENSION\] LABELS: an element's labels, an array.
    Labels,
    /// \[EXTENSION\] PATH_LENGTH: a path's edges, counted.
    PathLength,
    /// \[EXTENSION\] VERTICES: a path's nodes, an array.
    Vertices,
    /// \[EXTENSION\] EDGES: a path's edges, an array.
    Edges,
    /// \[EXTENSION\] IS_ACYCLIC: whether a path repeats no node.
    IsAcyclic,
    /// \[EXTENSION\] IS_TRAIL: whether a path repeats no edge.
    IsTrail,
    /// \[EXTENSION\] IS_SIMPLE: whether a path repeats no node but its
    /// first as its last.
    IsSimple,
    /// \[EXTENSION\] PATH_TO_JSON: a path binding, as JSON [P1 §7.1].
    PathToJson,
    /// \[EXTENSION\] PREV: an element variable's binding in the iteration
    /// before, of a quantified pattern; null in the first.
    Prev,
    /// \[EXTENSION\] FIRST: an element variable's binding in the first
    /// iteration of a quantified pattern.
    First,
}

/// Each function's name as written, in the order of `GraphFunctionName`.
pub const GRAPH_FUNCTIONS: [(GraphFunctionName, &str, VariableKind); 13] = [
    (GraphFunctionName::ElementId, "ELEMENT_ID", VariableKind::ElementOrPath),
    (GraphFunctionName::VertexId, "VERTEX_ID", VariableKind::Vertex),
    (GraphFunctionName::EdgeId, "EDGE_ID", VariableKind::Edge),
    (GraphFunctionName::Labels, "LABELS", VariableKind::Element),
    (GraphFunctionName::PathLength, "PATH_LENGTH", VariableKind::Path),
    (GraphFunctionName::Vertices, "VERTICES", VariableKind::Path),
    (GraphFunctionName::Edges, "EDGES", VariableKind::Path),
    (GraphFunctionName::IsAcyclic, "IS_ACYCLIC", VariableKind::Path),
    (GraphFunctionName::IsTrail, "IS_TRAIL", VariableKind::Path),
    (GraphFunctionName::IsSimple, "IS_SIMPLE", VariableKind::Path),
    (GraphFunctionName::PathToJson, "PATH_TO_JSON", VariableKind::Path),
    (GraphFunctionName::Prev, "PREV", VariableKind::Element),
    (GraphFunctionName::First, "FIRST", VariableKind::Element),
];

impl GraphFunctionName {
    /// The function of the name `word`, if one has it.
    #[must_use]
    pub fn named(word: &str) -> Option<GraphFunctionName> {
        GRAPH_FUNCTIONS.iter().find(|(_, w, _)| *w == word).map(|(f, _, _)| *f)
    }

    /// Its name as written.
    #[must_use]
    pub fn word(self) -> &'static str {
        GRAPH_FUNCTIONS.iter().find(|(f, _, _)| *f == self).map_or("", |(_, w, _)| w)
    }

    /// The kind of variable its operand names.
    #[must_use]
    pub fn operand(self) -> VariableKind {
        GRAPH_FUNCTIONS.iter().find(|(f, _, _)| *f == self).map_or(VariableKind::Element, |(_, _, k)| *k)
    }
}

/// A function of a graph variable: `<name>` ( `<variable>` ), and of
/// PREV and FIRST \[. `<property name>`\].
#[derive(Debug, Clone)]
pub struct GraphFunction {
    /// Which.
    pub name: GraphFunctionName,
    /// The variable it is of.
    pub operand: GraphVariable,
    /// Of PREV or FIRST, the property of the element it gives, if one is
    /// written after it.
    pub property: Option<String>,
}

/// Which spelling of the same predicate was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameKind {
    /// SAME, of elements.
    Same,
    /// \[EXTENSION\] VERTEX_EQUAL, of two nodes.
    VertexEqual,
    /// \[EXTENSION\] EDGE_EQUAL, of two edges.
    EdgeEqual,
}

/// Which end of an edge a node is tested to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathEnd {
    /// IS SOURCE OF.
    Source,
    /// IS DESTINATION OF.
    Destination,
}

/// [P1 §4.7] A predicate of graph elements.
#[derive(Debug, Clone)]
pub enum GraphPredicate {
    /// SAME ( `<element reference>` , ... ): whether all are bound to
    /// one element; or its spellings of two nodes or two edges.
    Same {
        /// The spelling.
        kind: SameKind,
        /// The elements, two at least.
        elements: Vec<GraphVariable>,
    },
    /// ALL_DIFFERENT ( `<element reference>` , ... ): whether no two are
    /// bound to one element.
    AllDifferent(Vec<GraphVariable>),
    /// PROPERTY_EXISTS ( `<element reference>` , `<property name>` ):
    /// whether the element has the property, null or not.
    PropertyExists {
        /// The element.
        element: GraphVariable,
        /// The property's name.
        property: String,
    },
    /// `<node>` IS \[NOT\] SOURCE OF `<edge>`, or DESTINATION OF.
    Endpoint {
        /// The node.
        node: GraphVariable,
        /// The edge.
        edge: GraphVariable,
        /// Which end.
        end: PathEnd,
        /// NOT was written.
        not: bool,
    },
    /// `<element>` IS \[NOT\] LABELED `<label expression>`, or
    /// `<element>` : `<label expression>`.
    Labeled {
        /// The element.
        element: GraphVariable,
        /// The label expression.
        label: LabelExpression,
        /// NOT was written.
        not: bool,
    },
    /// `<edge>` IS \[NOT\] DIRECTED.
    Directed {
        /// The edge.
        edge: GraphVariable,
        /// NOT was written.
        not: bool,
    },
    /// \[EXTENSION\] EXISTS IN PATH `<path variable>` ( `<path pattern
    /// expression>` \[WHERE `<search condition>`\] ): whether the pattern
    /// matches the subgraph of the path's elements, in path order. Its
    /// variables are its own.
    InPath {
        /// The path.
        path: GraphVariable,
        /// The pattern, written in parentheses.
        pattern: Box<ParenthesizedPath>,
    },
}

impl GraphPredicate {
    /// The variables it is of, each with the kind it must name.
    #[must_use]
    pub fn operands(&self) -> Vec<(&GraphVariable, VariableKind)> {
        match self {
            GraphPredicate::Same { kind, elements } => {
                let k = match kind {
                    SameKind::Same => VariableKind::Element,
                    SameKind::VertexEqual => VariableKind::Vertex,
                    SameKind::EdgeEqual => VariableKind::Edge,
                };
                elements.iter().map(|e| (e, k)).collect()
            }
            GraphPredicate::AllDifferent(elements) => elements.iter().map(|e| (e, VariableKind::Element)).collect(),
            GraphPredicate::PropertyExists { element, .. } | GraphPredicate::Labeled { element, .. } => {
                vec![(element, VariableKind::Element)]
            }
            GraphPredicate::Endpoint { node, edge, .. } => vec![(node, VariableKind::Vertex), (edge, VariableKind::Edge)],
            GraphPredicate::Directed { edge, .. } => vec![(edge, VariableKind::Edge)],
            GraphPredicate::InPath { path, .. } => vec![(path, VariableKind::Path)],
        }
    }

    /// Whether each operand must be an unconditional singleton (GRF-0023):
    /// those of SAME and ALL_DIFFERENT [P1 §4.7].
    #[must_use]
    pub fn needs_unconditional(&self) -> bool {
        matches!(self, GraphPredicate::Same { .. } | GraphPredicate::AllDifferent(_))
    }
}
