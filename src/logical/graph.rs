//! \[EXTENSION\] A GRAPH_TABLE bound [GS §8, §14]: what its search reads
//! of the graph's element tables, its path patterns as the binder hands
//! them to path search, the element variables and the values its
//! conditions and COLUMNS are expressions over, and its rows clause.
//!
//! Its conditions and columns are expressions over one row, its match
//! row: each element variable's columns, its properties then the hidden
//! columns of its identity (`VertexColumns`, `EdgeColumns`), then the
//! values computed of a match (`GraphValue`), then its operands, what it
//! reads of the rows before it and of enclosing queries, which the plan
//! evaluates of each row of its input (`Unnest::collections`). So it is
//! planned as a lateral derived table is.
//!
//! Citations: `[GS §n]` is the property graph implementation standard;
//! `[P1 §n]` Deutsch et al., "Graph Pattern Matching in GQL and SQL/PGQ"
//! (arXiv 2112.06217).

mod common;
mod identity;
mod indexes;
mod sizing;

use std::fmt::Write as _;
use std::sync::Arc;

use super::{AggCall, Cond, Expr, LogicalPlan};
use crate::catalog::QualifiedName;
use crate::graph::in_memory::InMemoryGraph;
use crate::graph::limits::OnLimit;
use crate::graph::program::{
    ElementTest, Filter, LabelExpr, MatchMode, PathPattern, Pattern, Predicate, Quantified, Read, Restrictor, Selector, VarId, VarKind,
    Variable,
};
use crate::graph::search::PathSearch;
use crate::graph::view::Orientation;

pub use identity::{ElementIdentity, IDENTITY_LENGTH, IdentityJson, identity_item, identity_text, identity_type};
pub use indexes::Pruning;
pub use sizing::GraphSizing;

/// The hidden columns of a vertex variable, after its properties: its
/// element identity and the number of its element table.
pub const VERTEX_COLUMNS: usize = 2;

/// The hidden columns of an edge variable, after its properties: its
/// identity, its element table's number, its source's and destination's
/// identities, and whether it is directed.
pub const EDGE_COLUMNS: usize = 5;

/// \[EXTENSION\] A GRAPH_TABLE, bound.
#[derive(Debug, Clone)]
pub struct GraphMatch {
    /// The property graph.
    pub graph: QualifiedName,
    /// The element tables it reads, numbered as `ElementId::table`.
    pub tables: Vec<ElementSource>,
    /// The graph's labels, numbered as `LabelId`.
    pub labels: Vec<String>,
    /// The path patterns.
    pub paths: Vec<PathShape>,
    /// The match mode [GS §7.3].
    pub mode: MatchMode,
    /// The element variables, in the order of their columns.
    pub vars: Vec<VarShape>,
    /// The prefilters [GS §6.8], each over the match row.
    pub prefilters: Vec<Prefilter>,
    /// The values computed of a match, after the variables' columns.
    pub slots: Vec<GraphValue>,
    /// The columns of the variables.
    pub width: usize,
    /// The operands, after the slots.
    pub operands: usize,
    /// The postfilter, the WHERE after the graph pattern [GS §6.8 R6.4].
    pub postfilter: Option<Cond>,
    /// Which rows a match gives [GS §8 R8.3].
    pub rows: RowsShape,
    /// COLUMNS, over the match row.
    pub columns: Vec<Expr>,
    /// The resource limits written in it [GS §20].
    pub options: GraphOptions,
    /// The optimizer hints written in it [GS §16 R16.8].
    pub hints: crate::ast::GraphHints,
    /// \[GS §16 R16.2\] Whether its rigid, cyclic paths are matched by a
    /// worst-case optimal join rather than searched (`graph::wcoj`).
    pub wcoj: bool,
    /// Its tables' sizes and the workers' settings, for EXPLAIN.
    pub sizing: GraphSizing,
}

/// \[EXTENSION\] How the rows of an element table are read.
#[derive(Debug, Clone)]
pub enum ElementRead {
    /// A base table, read as the statement's transaction sees it, a row
    /// by its id, and its edges through an adjacency index if it has one.
    Stored,
    /// A view, or a table read FOR SYSTEM_TIME: the rows of this plan,
    /// held for the statement, numbered as read. Its `Expr::Outer` slots
    /// are the GRAPH_TABLE's operands.
    Held(LogicalPlan),
}

/// \[EXTENSION\] The end of an edge table: its vertex table, the edge's
/// columns and the vertex's columns they match.
#[derive(Debug, Clone)]
pub struct EndShape {
    /// The vertex table, its number.
    pub table: usize,
    /// The columns of the edge.
    pub columns: Vec<usize>,
    /// The columns of the vertex each matches.
    pub references: Vec<usize>,
}

/// \[EXTENSION\] An edge table's ends.
#[derive(Debug, Clone)]
pub struct EdgeShape {
    /// Its source.
    pub source: EndShape,
    /// Its destination.
    pub destination: EndShape,
    /// Whether its edges are directed.
    pub directed: bool,
}

/// \[EXTENSION\] An element table of the graph, as its search reads it.
#[derive(Debug, Clone)]
pub struct ElementSource {
    /// Its alias in the graph.
    pub alias: String,
    /// The table or view.
    pub table: QualifiedName,
    /// Of an edge table, its ends.
    pub ends: Option<EdgeShape>,
    /// Its key's columns.
    pub key: Vec<usize>,
    /// Its labels.
    pub labels: Vec<u32>,
    /// Each of its properties, its name and value over a row of it.
    pub properties: Vec<(String, Expr)>,
    /// How its rows are read.
    pub read: ElementRead,
    /// The columns of a row of it.
    pub width: usize,
    /// Of a stored edge table, the adjacency index of its ends' columns
    /// its edges are followed through, if one is (`exec/graph_table`).
    pub adjacency: Option<String>,
    /// Of a stored table, the label index of its rows, if it has one
    /// (RULES.md D115).
    pub labels_index: Option<String>,
    /// Of a stored edge table, the reachability index of its ends'
    /// columns, if it has one (RULES.md D116).
    pub reachability: Option<String>,
}

impl ElementSource {
    /// The tables its rows are read from, added to `out` once each.
    pub(super) fn collect_tables(&self, out: &mut Vec<QualifiedName>) {
        let ElementRead::Held(plan) = &self.read else {
            if !out.contains(&self.table) {
                out.push(self.table.clone());
            }
            return;
        };
        plan.collect_tables(out);
    }
}

/// \[EXTENSION\] An element variable [GS §6.6]: where its columns are in
/// the match row, and how they are made of a row of each element table it
/// may bind.
#[derive(Debug, Clone)]
pub struct VarShape {
    /// Its name; an anonymous element's is made by normalization.
    pub name: String,
    /// Whether it binds edges.
    pub edge: bool,
    /// Its first column in the match row.
    pub offset: usize,
    /// Its columns: its properties, then its hidden columns.
    pub width: usize,
    /// Of each element table it may bind, the number and the expressions
    /// of its columns over a row of it.
    pub tables: Vec<(usize, Vec<Expr>)>,
    /// Each path pattern it is a variable of, and its number there.
    pub bound: Vec<(usize, VarId)>,
}

impl VarShape {
    /// The expressions of its columns over a row of element table `table`.
    pub fn projection(&self, table: usize) -> Option<&[Expr]> {
        self.tables.iter().find(|(t, _)| *t == table).map(|(_, p)| p.as_slice())
    }
}

/// \[EXTENSION\] An element test of a path pattern [GS §6.1, §6.3].
#[derive(Debug, Clone)]
pub struct TestShape {
    /// Its variable in the path pattern, none if anonymous.
    pub var: Option<VarId>,
    /// Its element variable, of `GraphMatch::vars`.
    pub shape: usize,
    /// Its label expression.
    pub labels: Option<LabelExpr>,
    /// Its prefilter, of `GraphMatch::prefilters`.
    pub filter: Option<usize>,
}

/// \[EXTENSION\] A path pattern expression, as `program::Pattern` holds
/// one, its prefilters numbered.
#[derive(Debug, Clone)]
pub enum PatternShape {
    /// A node pattern.
    Vertex(TestShape),
    /// An edge pattern.
    Edge(TestShape, Orientation),
    /// A concatenation.
    Concat(Vec<PatternShape>),
    /// `|`.
    Union(Vec<PatternShape>),
    /// `|+|`.
    Alternation(Vec<PatternShape>),
    /// A quantified pattern: its least and most iterations.
    Quantified(Box<PatternShape>, u32, Option<u32>),
    /// A parenthesized path pattern, its restrictor and prefilter.
    Group(Restrictor, Box<PatternShape>, Option<usize>),
}

/// \[EXTENSION\] A prefilter: its condition over the match row, the
/// element variable of the element it tests, if any, and the variables
/// it reads.
#[derive(Debug, Clone)]
pub struct Prefilter {
    /// The condition.
    pub cond: Cond,
    /// The element variable tested, of an element pattern's WHERE.
    pub own: Option<usize>,
    /// The element variables read, each with its number in the path
    /// pattern.
    pub reads: Vec<(usize, VarId)>,
}

/// \[EXTENSION\] The COST of a CHEAPEST selector [GS §7.2]: an expression
/// over the match row, the edge followed read as `var`.
#[derive(Debug, Clone)]
pub struct CostShape {
    /// The edge variable it reads, if any.
    pub var: Option<usize>,
    /// The cost.
    pub expr: Expr,
}

/// \[EXTENSION\] A path pattern, bound.
#[derive(Debug, Clone)]
pub struct PathShape {
    /// Its variables: each name, whether an edge's, and its element
    /// variable.
    pub variables: Vec<(String, bool, usize)>,
    /// Its selector.
    pub selector: Selector,
    /// The restrictor at its head.
    pub restrictor: Restrictor,
    /// Its expression.
    pub pattern: PatternShape,
    /// Its COST, of a CHEAPEST selector.
    pub cost: Option<CostShape>,
    /// The vertex tables a match may start at.
    pub starts: Vec<usize>,
}

impl PathShape {
    /// It as path search takes it, each prefilter the one `filter` makes
    /// of its number.
    pub fn instantiate(&self, make: &mut dyn FnMut(usize) -> Filter) -> PathPattern {
        let variables = self
            .variables
            .iter()
            .map(|(name, edge, _)| Variable { name: name.clone(), kind: if *edge { VarKind::Edge } else { VarKind::Vertex } })
            .collect();
        PathPattern::new(variables, instantiated(&self.pattern, make)).select(self.selector).restrict(self.restrictor)
    }
}

/// `shape` as path search takes it.
fn instantiated(shape: &PatternShape, filter: &mut dyn FnMut(usize) -> Filter) -> Pattern {
    let test = |t: &TestShape, make: &mut dyn FnMut(usize) -> Filter| ElementTest {
        var: t.var,
        labels: t.labels.clone(),
        filter: t.filter.map(&mut *make),
    };
    match shape {
        PatternShape::Vertex(t) => Pattern::Vertex(test(t, filter)),
        PatternShape::Edge(t, o) => Pattern::Edge(test(t, filter), *o),
        PatternShape::Concat(parts) => Pattern::Concat(parts.iter().map(|p| instantiated(p, filter)).collect()),
        PatternShape::Union(parts) => Pattern::Union(parts.iter().map(|p| instantiated(p, filter)).collect()),
        PatternShape::Alternation(parts) => Pattern::Alternation(parts.iter().map(|p| instantiated(p, filter)).collect()),
        PatternShape::Quantified(body, min, max) => {
            Pattern::Quantified(Box::new(Quantified { body: instantiated(body, filter), min: *min, max: *max }))
        }
        PatternShape::Group(restrictor, body, f) => Pattern::Group(Box::new(crate::graph::program::Group {
            restrictor: *restrictor,
            body: instantiated(body, filter),
            filter: f.map(&mut *filter),
        })),
    }
}

/// \[EXTENSION\] A path function [GS §6.9].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathFunction {
    /// PATH_LENGTH.
    Length,
    /// VERTICES.
    Vertices,
    /// EDGES.
    Edges,
    /// ELEMENT_ID of a path.
    Elements,
    /// IS_ACYCLIC.
    Acyclic,
    /// IS_TRAIL.
    Trail,
    /// IS_SIMPLE.
    Simple,
    /// PATH_TO_JSON.
    Json,
}

/// \[EXTENSION\] A value computed of a match, a column of the match row.
#[derive(Debug, Clone)]
pub enum GraphValue {
    /// A set function over a group variable's elements [P1 §4.4], its
    /// argument over the match row, the element in the variable's columns.
    Aggregate {
        /// The set function.
        call: Box<AggCall>,
        /// The group variable.
        var: usize,
    },
    /// A function of a path pattern's path.
    Path {
        /// The path pattern.
        path: usize,
        /// The function.
        function: PathFunction,
    },
}

/// \[EXTENSION\] The rows a match gives [GS §8 R8.3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowsShape {
    /// ONE ROW PER MATCH.
    Match,
    /// ONE ROW PER VERTEX: the element variable bound to each.
    Vertex(usize),
    /// ONE ROW PER STEP: the three element variables of each.
    Step(usize, usize, usize),
}

/// \[EXTENSION\] The resource limits a GRAPH_TABLE writes [GS §20], each
/// in place of its setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraphOptions {
    /// MAX_PATH_LENGTH.
    pub max_path_length: Option<u64>,
    /// MAX_PATHS_PER_PARTITION.
    pub max_paths_per_partition: Option<u64>,
    /// MAX_SEARCH_STATES.
    pub max_search_states: Option<u64>,
    /// ON LIMIT.
    pub on_limit: Option<OnLimit>,
    /// DETERMINISTIC_PATHS.
    pub deterministic: bool,
}

/// A prefilter for EXPLAIN alone, never called.
fn shown(reads: Vec<Read>) -> Filter {
    let test: Predicate = Arc::new(|_| Ok(true));
    Filter { reads, test }
}

impl GraphMatch {
    /// The reads of prefilter `k`, as path search takes them.
    pub fn reads(&self, k: usize) -> Vec<Read> {
        let reads = self.prefilters.get(k).map(|p| p.reads.as_slice()).unwrap_or_default();
        reads.iter().map(|&(_, var)| Read { var, previous: false }).collect()
    }

    /// \[GS §16 R16.6\] Of each path pattern, the earlier one it is
    /// written as, if one, whose bindings it takes (`graph/common.rs`).
    pub fn earlier(&self) -> Vec<Option<usize>> {
        common::earlier_paths(self)
    }

    /// \[GS §16 R16.9\] EXPLAIN of it: each path pattern's search and
    /// automaton, then its match mode and rows.
    pub fn render(&self) -> String {
        let mut out = format!("Graph table of {} ({} element tables)", self.graph, self.tables.len());
        let empty = InMemoryGraph::new();
        let earlier = self.earlier();
        for (n, path) in self.paths.iter().enumerate() {
            if let Some(m) = earlier.get(n).copied().flatten() {
                let _ = write!(out, "\n  path {}: as path {}, searched once", n.saturating_add(1), m.saturating_add(1));
                continue;
            }
            let pattern = path.instantiate(&mut |k| shown(self.reads(k)));
            let starts: Vec<&str> = path.starts.iter().filter_map(|&t| self.tables.get(t)).map(|t| t.alias.as_str()).collect();
            let _ = write!(out, "\n  path {}: from {}", n.saturating_add(1), starts.join(", "));
            match PathSearch::new(&pattern, &empty, self.sizing.options(self.hints.path_algo)) {
                Ok(search) => {
                    let size = self.sizing.size(self.tables.iter().map(|t| t.ends.is_some()));
                    let count = self.sizing.starts(&path.starts).filter(|_| size.is_some()).unwrap_or(starts.len().max(2));
                    let explained = search.explain_for(count, None, size);
                    for line in explained.lines() {
                        let _ = write!(out, "\n    {line}");
                    }
                }
                Err(e) => {
                    let _ = write!(out, "\n    {}", e.msg);
                }
            }
        }
        for t in self.tables.iter().filter(|t| t.ends.is_some()) {
            let how = t.adjacency.as_ref().map_or_else(|| "edges listed by a scan".to_owned(), |ix| format!("adjacency index {ix}"));
            let _ = write!(out, "\n  edge table {}: {how}", t.alias);
        }
        if self.wcoj {
            out.push_str("\n  worst-case optimal join of the paths' vertices, over their adjacency");
        }
        self.render_indexes(&mut out);
        let _ = write!(out, "\n  match mode {:?}, rows {:?}", self.mode, self.rows);
        if !self.hints.is_empty() {
            let _ = write!(out, "\n  hints {}", hints_text(&self.hints));
        }
        out
    }
}

/// \[GS §16 R16.8\] `hints` as EXPLAIN shows them, as written.
fn hints_text(hints: &crate::ast::GraphHints) -> String {
    let mut shown: Vec<String> = Vec::new();
    shown.extend(hints.start.iter().map(|(v, _)| format!("GRAPH_START({v})")));
    shown.extend(hints.reverse.iter().map(|(v, _)| format!("GRAPH_DIRECTION({v}, REVERSE)")));
    shown.extend(hints.no_wcoj.then(|| "NO_WCOJ".to_owned()));
    shown.extend(hints.use_adjacency.iter().map(|(t, _)| format!("USE_ADJACENCY({t})")));
    shown.extend(hints.path_algo.map(|a| format!("PATH_ALGO({})", a.word())));
    shown.join(", ")
}
