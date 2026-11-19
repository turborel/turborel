//! \[DESIGN\] How a search runs [GS §15, §17]: its algorithm and its
//! workers, chosen by its selector and restrictors, then by cost.
//!
//! The selector and restrictors name an algorithm's family. Of ANY
//! SHORTEST, or ANY, from many starts, MS-BFS is chosen when a batch's
//! starts, times the vertices each is estimated to reach, cover the graph,
//! so their searches meet and share their work; else BFS from each start,
//! whose states cost less. A start's reach is estimated from the graph's
//! size (`GraphView::size`): its average degree's powers summed, to the
//! search's MAX_PATH_LENGTH, at most every vertex. The degree is of the
//! edges the pattern's edge labels match, as the graph's statistics count
//! its label transitions, else of every edge. Without a size, MS-BFS is
//! chosen.
//!
//! Workers are used when there are more than one, the work is estimated
//! at GRAPH.PARALLEL_MIN_STATES states or more, and there is more than one
//! piece of it: the starts in morsels, or one start's levels. A hint,
//! PATH_ALGO [GS §16 R16.8], replaces the choice of algorithm within the
//! family, and is refused if no algorithm of it answers the pattern.

use std::fmt;

use super::{Algorithm, LANES, Parallelism, PathSearch, Plan};
use crate::ast::PathAlgorithm;
use crate::error::{Code, Result, SqlError};
use crate::graph::program::Selector;
use crate::graph::view::GraphSize;

/// \[DESIGN\] The most levels a start's reach is estimated over.
const MOST_LEVELS: usize = 64;

/// What a search's choice was made of, which EXPLAIN shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Estimate {
    /// Its starts.
    pub starts: usize,
    /// The states a search from one start is estimated to make.
    pub reach: u64,
    /// The graph's vertices.
    pub vertices: u64,
}

impl fmt::Display for Estimate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} starts, est. reach {} of {} vertices", self.starts, self.reach, self.vertices)
    }
}

/// The states a search from one start of a graph of `size` is estimated
/// to make, its walks at most `longest` edges long if given.
pub(super) fn reach(size: GraphSize, longest: Option<usize>) -> u64 {
    let vertices = size.vertices.max(1);
    let (mut total, mut level) = (1_u64, 1_u64);
    for _ in 0..longest.unwrap_or(MOST_LEVELS).min(MOST_LEVELS) {
        level = level.saturating_mul(size.edges).checked_div(vertices).unwrap_or(0);
        if level == 0 || total >= vertices {
            break;
        }
        total = total.saturating_add(level);
    }
    total.min(vertices)
}

/// Whether `algorithm` is of the breadth-first search of states of ANY
/// SHORTEST and ANY.
fn breadth_first(algorithm: Algorithm) -> bool {
    matches!(algorithm, Algorithm::Bfs | Algorithm::BidirectionalBfs | Algorithm::MultiSourceBfs)
}

/// The error of a PATH_ALGO hint no algorithm of which answers the pattern.
fn refused(hint: PathAlgorithm, why: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] GRF-0070 PATH_ALGO({}) cannot answer this pattern: {why} (R16.8)", hint.word()))
}

/// The algorithm of hint `hint` answering what `natural` answers, a
/// search of one start and one end if `point`.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0070) if none does.
fn hinted(natural: Algorithm, hint: PathAlgorithm, point: bool) -> Result<Algorithm> {
    let chosen = match hint {
        PathAlgorithm::Msbfs => breadth_first(natural).then_some(Algorithm::MultiSourceBfs),
        PathAlgorithm::Bibfs => (breadth_first(natural) && point).then_some(Algorithm::BidirectionalBfs),
        PathAlgorithm::Bfs => match natural {
            Algorithm::Bfs | Algorithm::BidirectionalBfs | Algorithm::MultiSourceBfs => Some(Algorithm::Bfs),
            Algorithm::PathBfs | Algorithm::AllShortest => Some(natural),
            Algorithm::KShortest | Algorithm::Dijkstra | Algorithm::Dfs | Algorithm::PathDijkstra => None,
        },
        PathAlgorithm::Dfs => (natural == Algorithm::Dfs).then_some(natural),
        PathAlgorithm::Dijkstra => matches!(natural, Algorithm::Dijkstra | Algorithm::PathDijkstra).then_some(natural),
    };
    chosen.ok_or_else(|| {
        let why = match hint {
            PathAlgorithm::Msbfs | PathAlgorithm::Bfs => "it searches by ANY SHORTEST, ANY or ALL SHORTEST only",
            PathAlgorithm::Bibfs => "it searches by ANY SHORTEST or ANY between one start and one end only",
            PathAlgorithm::Dfs => "it searches every path, of no selector or ANY k, only",
            PathAlgorithm::Dijkstra => "it searches by the CHEAPEST selectors only",
        };
        refused(hint, why)
    })
}

/// Of ANY SHORTEST from many starts, `natural` MS-BFS, BFS from each
/// start instead if the reaches of a batch's starts, by `estimate`, do not
/// cover the graph.
fn by_cost(natural: Algorithm, estimate: Option<Estimate>) -> Algorithm {
    let Some(e) = estimate.filter(|_| natural == Algorithm::MultiSourceBfs) else { return natural };
    let batch = u64::try_from(e.starts.min(LANES)).unwrap_or(u64::MAX);
    if batch.saturating_mul(e.reach) >= e.vertices { Algorithm::MultiSourceBfs } else { Algorithm::Bfs }
}

impl PathSearch<'_> {
    /// \[GS §15, §17\] What a search of `starts` starts, and `ends` ends if
    /// given, does on a graph of `size`, if known.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0070) of a PATH_ALGO hint that cannot answer it.
    pub fn plan_for(&self, starts: usize, ends: Option<usize>, size: Option<GraphSize>) -> Result<Plan> {
        let point = starts == 1 && ends == Some(1) && self.nfa.plain();
        let natural = self.algorithm(starts, point);
        // A step follows only the edges its labels match, if they were counted.
        let size = size.map(|s| GraphSize { edges: self.options.step_edges.unwrap_or(s.edges), ..s });
        let estimate =
            size.map(|size| Estimate { starts, reach: reach(size, self.options.limits.max_path_length), vertices: size.vertices });
        let algorithm = match self.options.algorithm {
            Some(hint) => hinted(natural, hint, point)?,
            None => by_cost(natural, estimate),
        };
        let parallelism = self.parallelism(algorithm, starts, estimate);
        Ok(Plan { algorithm, parallelism, estimate })
    }

    /// The algorithm of the selector and restrictors for `starts` starts,
    /// one start and one end if `point`.
    fn algorithm(&self, starts: usize, point: bool) -> Algorithm {
        if self.nfa.restricted() || self.selector == Selector::All {
            return match self.selector {
                Selector::All | Selector::Any(_) => Algorithm::Dfs,
                Selector::AnyCheapest | Selector::AllCheapest | Selector::Cheapest(_) => Algorithm::PathDijkstra,
                Selector::AnyShortest | Selector::AllShortest | Selector::Shortest(_) | Selector::ShortestGroups(_) => Algorithm::PathBfs,
            };
        }
        match self.selector {
            Selector::AnyShortest | Selector::Any(1) => {
                if point {
                    Algorithm::BidirectionalBfs
                } else if starts > 1 {
                    Algorithm::MultiSourceBfs
                } else {
                    Algorithm::Bfs
                }
            }
            Selector::AllShortest | Selector::ShortestGroups(_) => Algorithm::AllShortest,
            Selector::Shortest(_) | Selector::Any(_) => Algorithm::KShortest,
            Selector::AnyCheapest | Selector::AllCheapest | Selector::Cheapest(_) => Algorithm::Dijkstra,
            Selector::All => Algorithm::Dfs,
        }
    }

    /// How a search by `algorithm` of `starts` starts uses its workers:
    /// on them if its work, estimated as `estimate` says, pays for them.
    fn parallelism(&self, algorithm: Algorithm, starts: usize, estimate: Option<Estimate>) -> Parallelism {
        let width = self.options.threads.max(1);
        let one_batch = algorithm == Algorithm::MultiSourceBfs && starts <= LANES;
        let work = estimate.map_or(u64::MAX, |e| {
            let searches = if algorithm == Algorithm::MultiSourceBfs { starts.div_ceil(LANES) } else { starts };
            let each = if algorithm == Algorithm::MultiSourceBfs {
                e.reach.saturating_mul(u64::try_from(LANES).unwrap_or(u64::MAX)).min(e.vertices)
            } else {
                e.reach
            };
            u64::try_from(searches).unwrap_or(u64::MAX).saturating_mul(each)
        });
        let small = work < u64::try_from(self.options.parallel_min_states).unwrap_or(u64::MAX);
        if width <= 1 || algorithm == Algorithm::Dfs || one_batch || small {
            Parallelism::Sequential
        } else if starts > 1 {
            Parallelism::Morsels(width)
        } else if algorithm == Algorithm::Bfs {
            Parallelism::LevelSynchronous(width)
        } else {
            Parallelism::Sequential
        }
    }
}
