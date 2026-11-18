//! \[DESIGN\] PathSearch [GS §14 step 6, §15]: a path pattern's automaton
//! run over a graph from its start vertices, by the algorithm its
//! selector and restrictors call for, then its postfilter [GS §6.8 R6.4].
//!
//! Restrictors apply during the search [GS §7.1], selectors to each
//! (start, end) partition after it [GS §7.2 R7.1] [P1 §5.1], and the
//! postfilter to what the selector kept. Without a restrictor a search
//! walks states, a vertex and the threads at it; with one, paths
//! (`search/paths.rs`). A search with no selector, or ANY k over paths,
//! gives its paths as it finds them; the others once found.
//!
//! ANY, ANY k, ANY SHORTEST and SHORTEST k are not deterministic [GS §7.2
//! R7.2]: they give the first paths found. Edges are followed in the
//! order of their element ids, so the first found is the same on every
//! run; DETERMINISTIC_PATHS also orders the starts and each start's
//! paths by element ids.

mod bfs;
mod bidirectional;
mod groups;
mod multi;
mod paths;
mod ranked;
mod selection;
mod states;

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

pub use multi::LANES;

use super::binding::PathBinding;
use super::limits::{Area, Limits, Meter};
use super::nfa::Nfa;
use super::program::{CostHook, PathPattern, PostfilterHook, Selector};
use super::view::{ElementId, GraphView};
use crate::error::Result;
use crate::txn::Stop;
use paths::Dfs;
use states::{Context, Costs};

/// How a path search runs.
#[derive(Clone, Debug, Default)]
pub struct SearchOptions {
    /// Its resource limits [GS §20].
    pub limits: Limits,
    /// DETERMINISTIC_PATHS [GS §7.2 R7.2].
    pub deterministic: bool,
    /// The workers it may use, EXECUTOR.GRAPH_THREADS; 0 or 1 for none.
    pub threads: usize,
    /// What cancels it.
    pub stop: Option<Arc<Stop>>,
    /// The memory it may hold.
    pub area: Area,
}

/// \[GS §15\] The algorithm a search runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    /// Breadth-first search of states, for ANY SHORTEST and ANY.
    Bfs,
    /// Bidirectional breadth-first search, both ends bound.
    BidirectionalBfs,
    /// Breadth-first search from 64 starts at once.
    MultiSourceBfs,
    /// Breadth-first search keeping a predecessor DAG, for ALL SHORTEST
    /// and SHORTEST k GROUP.
    AllShortest,
    /// The k shortest walks, for SHORTEST k and ANY k.
    KShortest,
    /// Dijkstra's search, for the CHEAPEST selectors.
    Dijkstra,
    /// Depth-first search of paths, giving each as found.
    Dfs,
    /// Breadth-first search of paths, under a restrictor.
    PathBfs,
    /// Best-first search of paths by cost, under a restrictor.
    PathDijkstra,
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Algorithm::Bfs => "PathSearch-BFS",
            Algorithm::BidirectionalBfs => "PathSearch-BiBFS",
            Algorithm::MultiSourceBfs => "MS-BFS",
            Algorithm::AllShortest => "PathSearch-AllShort",
            Algorithm::KShortest => "PathSearch-KShort",
            Algorithm::Dijkstra => "PathSearch-Dijkstra",
            Algorithm::Dfs => "PathSearch-DFS",
            Algorithm::PathBfs => "PathSearch-BFS of paths",
            Algorithm::PathDijkstra => "PathSearch-Dijkstra of paths",
        })
    }
}

/// \[GS §17\] How a search uses its workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parallelism {
    /// On the calling thread alone.
    Sequential,
    /// Each level of one start's search on this many workers.
    LevelSynchronous(usize),
    /// The starts, in morsels, on this many workers.
    Morsels(usize),
}

/// What a search will do, for EXPLAIN [GS §16 R16.9].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Its algorithm.
    pub algorithm: Algorithm,
    /// How it uses its workers.
    pub parallelism: Parallelism,
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.parallelism {
            Parallelism::Sequential => write!(f, "{}", self.algorithm),
            Parallelism::LevelSynchronous(n) => write!(f, "{} (level-synchronous, {n} workers)", self.algorithm),
            Parallelism::Morsels(n) => write!(f, "{} (morsels of starts, {n} workers)", self.algorithm),
        }
    }
}

/// A path pattern compiled for searching a graph.
pub struct PathSearch<'v> {
    nfa: Nfa,
    selector: Selector,
    postfilter: Option<PostfilterHook>,
    cost: Option<CostHook>,
    view: &'v dyn GraphView,
    options: SearchOptions,
}

impl<'v> PathSearch<'v> {
    /// `pattern` compiled to search `view`.
    ///
    /// # Errors
    ///
    /// As `Nfa::compile`.
    pub fn new(pattern: &PathPattern, view: &'v dyn GraphView, options: SearchOptions) -> Result<PathSearch<'v>> {
        let nfa = Nfa::compile(pattern)?;
        Ok(PathSearch {
            nfa,
            selector: pattern.selector,
            postfilter: pattern.postfilter.clone(),
            cost: pattern.cost.clone(),
            view,
            options,
        })
    }

    /// Its automaton.
    pub fn automaton(&self) -> &Nfa {
        &self.nfa
    }

    /// What a search of `starts` starts, and `ends` ends if given, does.
    pub fn plan(&self, starts: usize, ends: Option<usize>) -> Plan {
        let width = self.options.threads.max(1);
        let algorithm = self.algorithm(starts, ends);
        // One batch of MS-BFS is one search; more are morsels of it.
        let one_batch = algorithm == Algorithm::MultiSourceBfs && starts <= LANES;
        let parallelism = if width <= 1 || algorithm == Algorithm::Dfs || one_batch {
            Parallelism::Sequential
        } else if starts > 1 {
            Parallelism::Morsels(width)
        } else if algorithm == Algorithm::Bfs {
            Parallelism::LevelSynchronous(width)
        } else {
            Parallelism::Sequential
        };
        Plan { algorithm, parallelism }
    }

    /// The algorithm for `starts` starts and `ends` ends.
    fn algorithm(&self, starts: usize, ends: Option<usize>) -> Algorithm {
        if self.nfa.restricted() || self.selector == Selector::All {
            return match self.selector {
                Selector::All | Selector::Any(_) => Algorithm::Dfs,
                Selector::AnyCheapest | Selector::AllCheapest | Selector::Cheapest(_) => Algorithm::PathDijkstra,
                Selector::AnyShortest | Selector::AllShortest | Selector::Shortest(_) | Selector::ShortestGroups(_) => Algorithm::PathBfs,
            };
        }
        match self.selector {
            Selector::AnyShortest | Selector::Any(1) => {
                let point = starts == 1 && ends == Some(1) && self.nfa.plain();
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

    /// \[GS §16 R16.9\] EXPLAIN of a search of `starts` starts and `ends`
    /// ends: its plan, then its automaton.
    pub fn explain(&self, starts: usize, ends: Option<usize>) -> String {
        format!("{}\n{}", self.plan(starts, ends), self.nfa)
    }

    /// The context of a search ending at `ends`, if given.
    fn context(&self, ends: Option<Arc<HashSet<ElementId>>>) -> Context<'_> {
        let cost = match (&self.cost, self.selector.by_cost()) {
            (_, false) => Costs::Length,
            (Some(hook), true) => Costs::Hook(&hook.0),
            (None, true) => Costs::View,
        };
        Context { nfa: &self.nfa, view: self.view, cost, ends }
    }

    /// The paths matched from `starts` to `ends`, if given, each once.
    ///
    /// # Errors
    ///
    /// Any error of the view, a prefilter or the postfilter, a cost
    /// (GRF-0034), a limit (GRF-0061, -918), or a cancel (-920).
    pub fn run(&self, starts: &[ElementId], ends: Option<&[ElementId]>) -> Result<Matches<'_>> {
        let starts = self.order(starts);
        let plan = self.plan(starts.len(), ends.map(<[ElementId]>::len));
        let ctx = self.context(ends.map(|e| Arc::new(e.iter().copied().collect())));
        let meter = Meter::new(self.options.limits, self.options.stop.clone(), self.options.area.clone());
        let postfilter = self.postfilter.as_ref().map(|p| &p.0);
        if plan.algorithm == Algorithm::Dfs {
            let dfs = Dfs::new(ctx, self.selector, starts, meter);
            return Ok(Matches { inner: Inner::Streaming(Box::new(dfs)), postfilter, truncated: false });
        }
        let mut meter = meter;
        let mut found = match plan.parallelism {
            Parallelism::Morsels(width) => {
                let size = if plan.algorithm == Algorithm::MultiSourceBfs { LANES } else { 1 };
                let parts = crate::threads::morsels(width, (starts.len(), size), meter.stop(), |range| {
                    let mut part = meter.part();
                    self.sequential(&ctx, (starts.get(range).unwrap_or_default(), plan), &mut part)
                })?;
                parts.into_iter().flatten().collect()
            }
            Parallelism::Sequential | Parallelism::LevelSynchronous(_) => self.sequential(&ctx, (&starts, plan), &mut meter)?,
        };
        if self.options.deterministic {
            for paths in &mut found {
                paths.sort_by(|a: &PathBinding, b: &PathBinding| {
                    (a.end(), a.length(), &a.elements).cmp(&(b.end(), b.length(), &b.elements))
                });
            }
        }
        meter.warn();
        let truncated = meter.truncated();
        Ok(Matches { inner: Inner::Ready(found.into_iter().flatten().collect::<Vec<_>>().into_iter()), postfilter, truncated })
    }

    /// `starts`, each once: in element order if deterministic.
    fn order(&self, starts: &[ElementId]) -> Vec<ElementId> {
        let mut seen = HashSet::new();
        let mut ordered: Vec<ElementId> = starts.iter().copied().filter(|s| seen.insert(*s)).collect();
        if self.options.deterministic {
            ordered.sort_unstable();
        }
        ordered
    }

    /// The paths of each of `starts`, searched on this thread by `plan`.
    fn sequential(&self, ctx: &Context<'_>, (starts, plan): (&[ElementId], Plan), meter: &mut Meter) -> Result<Vec<Vec<PathBinding>>> {
        if plan.algorithm == Algorithm::MultiSourceBfs {
            let mut all = Vec::new();
            for batch in starts.chunks(LANES) {
                if meter.truncated() {
                    break;
                }
                all.extend(multi::multi_source(ctx, batch, meter)?);
            }
            return Ok(all);
        }
        // A search truncated by a limit ends: the starts after get nothing.
        let mut all = Vec::with_capacity(starts.len());
        for &start in starts {
            if meter.truncated() {
                break;
            }
            all.push(self.one(ctx, start, plan, meter)?);
        }
        Ok(all)
    }

    /// The paths of `start`, by `plan`.
    fn one(&self, ctx: &Context<'_>, start: ElementId, plan: Plan, meter: &mut Meter) -> Result<Vec<PathBinding>> {
        match (plan.algorithm, plan.parallelism) {
            (Algorithm::Bfs, Parallelism::LevelSynchronous(width)) => bfs::any_shortest_levels(ctx, start, width, meter),
            (Algorithm::Bfs | Algorithm::MultiSourceBfs, _) => bfs::any_shortest(ctx, start, meter),
            (Algorithm::BidirectionalBfs, _) => match ctx.ends.as_ref().and_then(|e| e.iter().next().copied()) {
                Some(end) => bidirectional::bidirectional(ctx, (start, end), meter),
                None => bfs::any_shortest(ctx, start, meter),
            },
            (Algorithm::AllShortest, _) => groups::groups(ctx, start, self.selector.k(), meter),
            (Algorithm::KShortest | Algorithm::Dijkstra, _) => ranked::ranked(ctx, start, self.selector, meter),
            (Algorithm::PathBfs | Algorithm::PathDijkstra | Algorithm::Dfs, _) => paths::best_first(ctx, start, self.selector, meter),
        }
    }
}

/// How a search gives its paths.
enum Inner<'s> {
    /// All found already.
    Ready(std::vec::IntoIter<PathBinding>),
    /// Found as they are asked for.
    Streaming(Box<Dfs<'s>>),
}

/// The paths a search matched, its postfilter applied [GS §6.8 R6.4].
pub struct Matches<'s> {
    inner: Inner<'s>,
    postfilter: Option<&'s super::program::Postfilter>,
    truncated: bool,
}

impl Matches<'_> {
    /// Whether a limit ended the search ON LIMIT TRUNCATE [GS §20].
    pub fn truncated(&self) -> bool {
        match &self.inner {
            Inner::Ready(_) => self.truncated,
            Inner::Streaming(dfs) => dfs.meter.truncated(),
        }
    }

    /// The next path, before the postfilter.
    fn unfiltered(&mut self) -> Result<Option<PathBinding>> {
        match &mut self.inner {
            Inner::Ready(paths) => Ok(paths.next()),
            Inner::Streaming(dfs) => {
                let next = dfs.next_path()?;
                if next.is_none() {
                    dfs.meter.warn();
                }
                Ok(next)
            }
        }
    }
}

impl Iterator for Matches<'_> {
    type Item = Result<PathBinding>;

    fn next(&mut self) -> Option<Result<PathBinding>> {
        loop {
            let path = match self.unfiltered() {
                Ok(Some(path)) => path,
                Ok(None) => return None,
                Err(e) => return Some(Err(e)),
            };
            match self.postfilter.map_or(Ok(true), |keep| keep(&path)) {
                Ok(true) => return Some(Ok(path)),
                Ok(false) => {}
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests;
