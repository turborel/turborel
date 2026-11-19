//! \[EXTENSION\] The library's functions as SQL invokes them (RULES.md
//! D145): each algorithm's name in schema GRAPH_ALGO, its
//! parameters and its result's columns, and a run of one on the rows of
//! its table arguments, each vertex its key value (`keys.rs`).
//!
//! An invocation's first argument is the graph: a table argument whose
//! first two columns are each edge's source and destination keys. The
//! parameter VERTICES, a table argument too, adds the vertices its first
//! column holds, so a vertex of no edge is one. A graph's name in place of
//! the table is the seam a graph of the catalog plugs in at: its edge
//! tables' endpoint keys are the same two columns.

use super::keys::Keys;
use super::{
    Betweenness, Cancel, Control, Jaccard, LabelPropagation, Louvain, PageRank, UNREACHED, Undirected, Vertex, betweenness, bfs_levels,
    graph, jaccard, k_core, label_propagation, local_clustering, louvain, pagerank, scc, sssp, triangle_count, wcc,
};
use crate::error::{Code, Result, SqlError};
use crate::storage::Row;
use crate::value::{Decimal, Value};

/// The schema the library's functions are of.
pub const SCHEMA: &str = "GRAPH_ALGO";

/// \[EXTENSION\] An algorithm of the library [GS §18].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    /// PAGERANK.
    PageRank,
    /// WCC.
    Wcc,
    /// SCC.
    Scc,
    /// BFS_LEVELS.
    BfsLevels,
    /// SSSP.
    Sssp,
    /// TRIANGLE_COUNT.
    TriangleCount,
    /// LOCAL_CLUSTERING.
    LocalClustering,
    /// BETWEENNESS.
    Betweenness,
    /// LOUVAIN.
    Louvain,
    /// LABEL_PROPAGATION.
    LabelPropagation,
    /// K_CORE.
    KCore,
    /// JACCARD_SIMILARITY.
    Jaccard,
}

/// Every algorithm, in the order the standard lists them.
pub const ALGORITHMS: [Algorithm; 12] = [
    Algorithm::PageRank,
    Algorithm::Wcc,
    Algorithm::Scc,
    Algorithm::BfsLevels,
    Algorithm::Sssp,
    Algorithm::TriangleCount,
    Algorithm::LocalClustering,
    Algorithm::Betweenness,
    Algorithm::Louvain,
    Algorithm::LabelPropagation,
    Algorithm::KCore,
    Algorithm::Jaccard,
];

/// What a parameter takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The edges: a table argument.
    Edges,
    /// More vertices: a table argument, its first column their keys.
    Vertices,
    /// A vertex's key.
    Start,
    /// A number, of DOUBLE PRECISION.
    Number,
    /// A count, a whole number not negative.
    Count,
    /// The name of the edges' column of each edge's cost, as text.
    Cost,
}

/// A parameter: its name and what it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parameter {
    /// Its name, which a named argument gives.
    pub name: &'static str,
    /// What it takes.
    pub kind: Kind,
}

/// The type of a result's value column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// DOUBLE PRECISION.
    Real,
    /// BIGINT.
    Count,
    /// A vertex's key, of the type of the edges' source.
    Key,
}

/// `name` of `kind`.
const fn p(name: &'static str, kind: Kind) -> Parameter {
    Parameter { name, kind }
}

const EDGES: Parameter = p("EDGES", Kind::Edges);
const VERTICES: Parameter = p("VERTICES", Kind::Vertices);
const START: Parameter = p("START", Kind::Start);
const COST: Parameter = p("COST", Kind::Cost);
const SEED: Parameter = p("SEED", Kind::Count);
const ITERATIONS: Parameter = p("ITERATIONS", Kind::Count);
const TOLERANCE: Parameter = p("TOLERANCE", Kind::Number);
const DAMPING: Parameter = p("DAMPING", Kind::Number);
const SAMPLE: Parameter = p("SAMPLE", Kind::Count);
const RESOLUTION: Parameter = p("RESOLUTION", Kind::Number);
const LEVELS: Parameter = p("LEVELS", Kind::Count);
const TOP_K: Parameter = p("TOP_K", Kind::Count);
const THRESHOLD: Parameter = p("THRESHOLD", Kind::Number);

impl Algorithm {
    /// Its name in schema GRAPH_ALGO.
    pub fn name(self) -> &'static str {
        match self {
            Algorithm::PageRank => "PAGERANK",
            Algorithm::Wcc => "WCC",
            Algorithm::Scc => "SCC",
            Algorithm::BfsLevels => "BFS_LEVELS",
            Algorithm::Sssp => "SSSP",
            Algorithm::TriangleCount => "TRIANGLE_COUNT",
            Algorithm::LocalClustering => "LOCAL_CLUSTERING",
            Algorithm::Betweenness => "BETWEENNESS",
            Algorithm::Louvain => "LOUVAIN",
            Algorithm::LabelPropagation => "LABEL_PROPAGATION",
            Algorithm::KCore => "K_CORE",
            Algorithm::Jaccard => "JACCARD_SIMILARITY",
        }
    }

    /// The algorithm named `name`, if one is.
    pub fn named(name: &str) -> Option<Algorithm> {
        ALGORITHMS.into_iter().find(|a| a.name() == name)
    }

    /// Its parameters, in order: the edges first, the vertices last.
    pub fn parameters(self) -> &'static [Parameter] {
        match self {
            Algorithm::PageRank => &[EDGES, DAMPING, ITERATIONS, TOLERANCE, VERTICES],
            Algorithm::Wcc | Algorithm::Scc | Algorithm::TriangleCount | Algorithm::LocalClustering | Algorithm::KCore => {
                &[EDGES, VERTICES]
            }
            Algorithm::BfsLevels => &[EDGES, START, VERTICES],
            Algorithm::Sssp => &[EDGES, START, COST, VERTICES],
            Algorithm::Betweenness => &[EDGES, SAMPLE, SEED, VERTICES],
            Algorithm::Louvain => &[EDGES, RESOLUTION, LEVELS, TOLERANCE, SEED, COST, VERTICES],
            Algorithm::LabelPropagation => &[EDGES, ITERATIONS, VERTICES],
            Algorithm::Jaccard => &[EDGES, TOP_K, THRESHOLD, VERTICES],
        }
    }

    /// Its result's columns after the vertex's key, or two keys of a pair,
    /// and their types.
    pub fn result(self) -> &'static [(&'static str, Output)] {
        match self {
            Algorithm::PageRank => &[("VERTEX", Output::Key), ("RANK", Output::Real)],
            Algorithm::Wcc | Algorithm::Scc => &[("VERTEX", Output::Key), ("COMPONENT", Output::Key)],
            Algorithm::BfsLevels => &[("VERTEX", Output::Key), ("DEPTH", Output::Count)],
            Algorithm::Sssp => &[("VERTEX", Output::Key), ("DISTANCE", Output::Real)],
            Algorithm::TriangleCount => &[("VERTEX", Output::Key), ("TRIANGLES", Output::Count)],
            Algorithm::LocalClustering => &[("VERTEX", Output::Key), ("COEFFICIENT", Output::Real)],
            Algorithm::Betweenness => &[("VERTEX", Output::Key), ("CENTRALITY", Output::Real)],
            Algorithm::Louvain | Algorithm::LabelPropagation => &[("VERTEX", Output::Key), ("COMMUNITY", Output::Key)],
            Algorithm::KCore => &[("VERTEX", Output::Key), ("CORE", Output::Count)],
            Algorithm::Jaccard => &[("VERTEX1", Output::Key), ("VERTEX2", Output::Key), ("SIMILARITY", Output::Real)],
        }
    }

    /// Whether its graph's edges have no direction, as an undirected
    /// algorithm reads them.
    fn undirected(self) -> bool {
        match self {
            Algorithm::TriangleCount
            | Algorithm::LocalClustering
            | Algorithm::Louvain
            | Algorithm::LabelPropagation
            | Algorithm::KCore
            | Algorithm::Jaccard => true,
            Algorithm::PageRank | Algorithm::Wcc | Algorithm::Scc | Algorithm::BfsLevels | Algorithm::Sssp | Algorithm::Betweenness => {
                false
            }
        }
    }
}

/// The values of a run's arguments, each by its parameter's name, none
/// of a parameter given no argument or NULL.
#[derive(Debug, Clone, Default)]
pub struct Arguments(pub Vec<(&'static str, Value)>);

impl Arguments {
    /// The value of parameter `name`, if given.
    fn get(&self, name: &str) -> Option<&Value> {
        self.0.iter().find(|(n, v)| *n == name && !v.is_null()).map(|(_, v)| v)
    }

    /// The number of `name`, else `default`.
    fn number(&self, name: &str, default: f64) -> f64 {
        self.get(name).and_then(Value::as_f64).unwrap_or(default)
    }

    /// The count of `name`, if given.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is negative or not whole.
    fn count(&self, algorithm: Algorithm, name: &str) -> Result<Option<u64>> {
        let Some(v) = self.get(name) else { return Ok(None) };
        let units = if let Value::Exact(d) = v { (d.scale == 0).then_some(d.units) } else { None };
        units
            .and_then(|u| u64::try_from(u).ok())
            .map(Some)
            .ok_or_else(|| super::invalid(algorithm.name(), name, "a whole number not negative"))
    }

    /// The count of `name` as a `u32`, else `default`.
    fn count_u32(&self, algorithm: Algorithm, name: &str, default: u32) -> Result<u32> {
        Ok(self.count(algorithm, name)?.map_or(default, |c| u32::try_from(c).unwrap_or(u32::MAX)))
    }
}

/// The rows of a run's table arguments: its edges, its vertices if
/// given, and the edges' cost column if named.
pub struct Input<'r> {
    /// The edges' rows, each's source and destination keys first.
    pub edges: &'r [Row],
    /// The vertices' rows, each's key first.
    pub vertices: Option<&'r [Row]>,
    /// The place of the edges' column of each edge's cost.
    pub cost: Option<usize>,
}

/// What a run read and gave: the graph's vertices and edges and its
/// result's rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sizes {
    /// The graph's vertices.
    pub vertices: usize,
    /// Its edges, each an edge row of two keys not null.
    pub edges: usize,
}

/// [GS §18] `algorithm` run on `input` with `arguments` under `control`:
/// its result's rows, each a vertex's key and its value, or a pair's two
/// keys and their similarity, in the order of the vertices' keys; and the
/// graph's sizes.
///
/// # Errors
///
/// SQLCODE -301 if an argument is out of its range, or a cost is not a
/// number, or negative where the algorithm takes none (GRF-0034); -918
/// past the budget; the cancel's error once told to stop.
pub fn run<C: Cancel>(
    algorithm: Algorithm,
    arguments: &Arguments,
    input: &Input<'_>,
    control: &Control<'_, C>,
) -> Result<(Vec<Row>, Sizes)> {
    let keys = Keys::of(input.edges, input.vertices, control.memory)?;
    let weighted = input.cost.is_some();
    let negative = algorithm == Algorithm::Sssp;
    let _edges_held = control.memory.items::<(Vertex, Vertex, f64)>(input.edges.len(), "the edges")?;
    let edges = keys.edges(input.edges, input.cost, negative)?;
    let sizes = Sizes { vertices: keys.len(), edges: edges.len() };
    let g = graph(control.memory, (keys.len(), weighted), edges)?;
    let rows = if algorithm.undirected() {
        undirected(algorithm, arguments, &keys, &g.undirected(control)?, control)?
    } else {
        directed(algorithm, arguments, &keys, &g, control)?
    };
    Ok((rows, sizes))
}

/// A real value.
fn real(x: f64) -> Value {
    Value::Approx(x)
}

/// A count's value.
fn count(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The directed algorithm `algorithm` run on `g`.
fn directed<C: Cancel>(
    algorithm: Algorithm,
    arguments: &Arguments,
    keys: &Keys<'_>,
    g: &super::Graph<'_>,
    control: &Control<'_, C>,
) -> Result<Vec<Row>> {
    let start = || arguments.get(START.name).and_then(|s| keys.find(s));
    Ok(match algorithm {
        Algorithm::PageRank => {
            let defaults = PageRank::default();
            let p = PageRank {
                damping: arguments.number(DAMPING.name, defaults.damping),
                iterations: arguments.count_u32(algorithm, ITERATIONS.name, defaults.iterations)?,
                tolerance: arguments.number(TOLERANCE.name, defaults.tolerance),
            };
            keys.valued(pagerank(g, &p, control)?.ranks.into_iter().map(|r| Some(real(r))))
        }
        Algorithm::Wcc => keys.labelled(&wcc(g, control)?),
        Algorithm::Scc => keys.labelled(&scc(g, control)?),
        Algorithm::BfsLevels => match start() {
            Some(s) => keys.valued(bfs_levels(g, s, control)?.into_iter().map(|l| (l != UNREACHED).then(|| count(u64::from(l))))),
            None => Vec::new(),
        },
        Algorithm::Sssp => match start() {
            Some(s) => keys.valued(sssp(g, s, control)?.into_iter().map(|d| d.is_finite().then(|| real(d)))),
            None => Vec::new(),
        },
        Algorithm::Betweenness => {
            let sample = arguments.count(algorithm, SAMPLE.name)?.map(|k| usize::try_from(k).unwrap_or(usize::MAX));
            let p = Betweenness { sample, seed: arguments.count(algorithm, SEED.name)?.unwrap_or_default() };
            keys.valued(betweenness(g, &p, control)?.into_iter().map(|c| Some(real(c))))
        }
        Algorithm::TriangleCount
        | Algorithm::LocalClustering
        | Algorithm::Louvain
        | Algorithm::LabelPropagation
        | Algorithm::KCore
        | Algorithm::Jaccard => return Err(SqlError::new(Code::Api, "[DESIGN] an undirected algorithm runs on the undirected graph")),
    })
}

/// The undirected algorithm `algorithm` run on `u`.
fn undirected<C: Cancel>(
    algorithm: Algorithm,
    arguments: &Arguments,
    keys: &Keys<'_>,
    u: &Undirected<'_>,
    control: &Control<'_, C>,
) -> Result<Vec<Row>> {
    Ok(match algorithm {
        Algorithm::TriangleCount => keys.valued(triangle_count(u, control)?.into_iter().map(|t| Some(count(t)))),
        Algorithm::LocalClustering => keys.valued(local_clustering(u, &triangle_count(u, control)?).into_iter().map(|c| Some(real(c)))),
        Algorithm::Louvain => {
            let defaults = Louvain::default();
            let p = Louvain {
                resolution: arguments.number(RESOLUTION.name, defaults.resolution),
                levels: arguments.count_u32(algorithm, LEVELS.name, defaults.levels)?,
                tolerance: arguments.number(TOLERANCE.name, defaults.tolerance),
                seed: arguments.count(algorithm, SEED.name)?.unwrap_or(defaults.seed),
            };
            keys.labelled(&louvain(u, &p, control)?.labels)
        }
        Algorithm::LabelPropagation => {
            let p =
                LabelPropagation { iterations: arguments.count_u32(algorithm, ITERATIONS.name, LabelPropagation::default().iterations)? };
            keys.labelled(&label_propagation(u, &p, control)?.0)
        }
        Algorithm::KCore => keys.valued(k_core(u, control)?.into_iter().map(|k| Some(count(u64::from(k))))),
        Algorithm::Jaccard => {
            let top_k = arguments.count(algorithm, TOP_K.name)?.map(|k| usize::try_from(k).unwrap_or(usize::MAX));
            let p = Jaccard { top_k, threshold: arguments.number(THRESHOLD.name, Jaccard::default().threshold) };
            keys.paired(jaccard(u, &p, control)?)
        }
        Algorithm::PageRank | Algorithm::Wcc | Algorithm::Scc | Algorithm::BfsLevels | Algorithm::Sssp | Algorithm::Betweenness => {
            return Err(SqlError::new(Code::Api, "[DESIGN] a directed algorithm runs on the directed graph"));
        }
    })
}
