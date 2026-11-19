//! \[DESIGN\] The Vamana indexing algorithm in memory [DiskANN, NeurIPS
//! 2019 Algorithm 3]: start from a random R-regular graph and the medoid
//! s; for each point in a random order, GreedySearch(s, x, 1, L) gives
//! the visited set V, RobustPrune(x, V, α, R) its out-edges, and each new
//! neighbour j gains the back edge to x, pruned again if past R. Two
//! passes, "the first pass with α = 1, and the second with a user-defined
//! α ≥ 1" [§2.3]. Used for a whole build that fits memory, and for each
//! cluster of the merged build.
//!
//! The medoid is taken as the point nearest the centroid, as the exact
//! medoid costs a pass of n² distances.

use crate::error::Result;

use super::Metric;
use super::finite;
use super::frontier::{Entry, Frontier, Visited};
use super::meta::NO_NODE;
use super::node::corrupt;
use super::prune::{Bound, Candidate, Vectors, robust_prune};
use super::random::Random;

/// An adjacency list of bounded degree, `degree` slots a node.
pub(super) struct Graph {
    pub(super) degree: usize,
    edges: Vec<u32>,
    counts: Vec<usize>,
}

impl Graph {
    /// A graph of `n` nodes and no edges.
    pub(super) fn new(n: usize, degree: usize) -> Graph {
        Graph { degree, edges: vec![0; n.saturating_mul(degree)], counts: vec![0; n] }
    }

    /// The out-neighbours of `id`.
    pub(super) fn neighbours(&self, id: u32) -> &[u32] {
        let at = usize::try_from(id).unwrap_or(usize::MAX);
        let count = self.counts.get(at).copied().unwrap_or(0);
        let start = at.saturating_mul(self.degree);
        self.edges.get(start..start.saturating_add(count)).unwrap_or_default()
    }

    /// Makes `list` (at most `degree`) the out-neighbours of `id`.
    pub(super) fn set(&mut self, id: u32, list: &[u32]) {
        let at = usize::try_from(id).unwrap_or(usize::MAX);
        let start = at.saturating_mul(self.degree);
        let kept = list.get(..self.degree).unwrap_or(list);
        if let Some(slots) = self.edges.get_mut(start..start.saturating_add(kept.len())) {
            slots.copy_from_slice(kept);
        }
        if let Some(count) = self.counts.get_mut(at) {
            *count = kept.len();
        }
    }

    /// The nodes.
    pub(super) fn len(&self) -> usize {
        self.counts.len()
    }
}

/// What a build is asked for.
#[derive(Debug, Clone, Copy)]
pub(super) struct Build {
    pub(super) list: usize,
    pub(super) alpha: f64,
    pub(super) passes: u8,
    pub(super) degree: usize,
    pub(super) seed: u64,
}

/// A Vamana graph of `vectors`, and its medoid (`NO_NODE` if empty).
///
/// # Errors
///
/// SQLCODE -910 for a distance that is not finite.
pub(super) fn build(vectors: Vectors<'_>, build: Build, metric: &dyn Metric) -> Result<(Graph, u32)> {
    let n = vectors.data.len().checked_div(vectors.dims).unwrap_or(0);
    let mut graph = Graph::new(n, build.degree);
    if n == 0 {
        return Ok((graph, NO_NODE));
    }
    let medoid = medoid(vectors, n, metric)?;
    let mut random = Random::new(build.seed);
    random_graph(&mut graph, &mut random);
    let mut order: Vec<u32> = (0..u32::try_from(n).unwrap_or(u32::MAX)).collect();
    random.shuffle(&mut order);
    let alphas: &[f64] = if build.passes >= 2 { &[1.0, build.alpha] } else { &[build.alpha] };
    let mut walk = Walk { visited: Visited::new(), frontier: Frontier::new(build.list), expanded: Vec::new(), batch: Vec::new() };
    for &alpha in alphas {
        let bound = Bound { alpha, degree: build.degree };
        for &point in &order {
            insert(&mut graph, point, (medoid, bound, build.list), (vectors, metric), &mut walk)?;
        }
    }
    Ok((graph, medoid))
}

/// One point's step of Algorithm 3.
fn insert(
    graph: &mut Graph,
    point: u32,
    (start, bound, list): (u32, Bound, usize),
    (vectors, metric): (Vectors<'_>, &dyn Metric),
    walk: &mut Walk,
) -> Result<()> {
    let x = vectors.row(row(point))?;
    greedy(graph, start, x, (list, vectors, metric), walk)?;
    let mut candidates = std::mem::take(&mut walk.expanded);
    for &n in graph.neighbours(point) {
        candidates.push(Candidate { distance: finite(metric.distance(x, vectors.row(row(n))?), "metric")?, id: n, row: row(n) });
    }
    let kept = robust_prune(point, &mut candidates, vectors, bound, metric)?;
    graph.set(point, &kept);
    walk.expanded = candidates;
    for &j in &kept {
        back_edge(graph, j, point, (vectors, bound, metric))?;
    }
    Ok(())
}

/// Adds the edge (j, x), pruning N_out(j) ∪ {x} if past R [Algorithm 3].
pub(super) fn back_edge(graph: &mut Graph, j: u32, x: u32, (vectors, bound, metric): (Vectors<'_>, Bound, &dyn Metric)) -> Result<()> {
    let list = graph.neighbours(j);
    if list.contains(&x) {
        return Ok(());
    }
    if list.len() < graph.degree {
        let mut more = list.to_vec();
        more.push(x);
        graph.set(j, &more);
        return Ok(());
    }
    let from = vectors.row(row(j))?;
    let mut candidates = Vec::with_capacity(list.len().saturating_add(1));
    for &n in list.iter().chain(std::iter::once(&x)) {
        candidates.push(Candidate { distance: finite(metric.distance(from, vectors.row(row(n))?), "metric")?, id: n, row: row(n) });
    }
    let kept = robust_prune(j, &mut candidates, vectors, bound, metric)?;
    graph.set(j, &kept);
    Ok(())
}

/// A greedy search's reusable state.
struct Walk {
    visited: Visited,
    frontier: Frontier,
    /// The visited set V, with distances to the query.
    expanded: Vec<Candidate>,
    batch: Vec<Entry>,
}

/// GreedySearch(s, x, 1, L) [Algorithm 1]: fills `walk.expanded` with V.
fn greedy(graph: &Graph, start: u32, x: &[f32], (list, vectors, metric): (usize, Vectors<'_>, &dyn Metric), walk: &mut Walk) -> Result<()> {
    walk.visited.start(graph.len(), None)?;
    walk.frontier.reset(list);
    walk.expanded.clear();
    walk.visited.visit(start);
    walk.frontier.add(finite(metric.distance(x, vectors.row(row(start))?), "metric")?, start);
    loop {
        walk.frontier.take(1, &mut walk.batch);
        let Some(&Entry { distance, id: p, .. }) = walk.batch.first() else { return Ok(()) };
        walk.expanded.push(Candidate { distance, id: p, row: row(p) });
        if metric.batches() {
            expand_batched((graph.neighbours(p), x), (vectors, metric), walk)?;
            continue;
        }
        for &n in graph.neighbours(p) {
            if walk.visited.visit(n) {
                let d = finite(metric.distance(x, vectors.row(row(n))?), "metric")?;
                walk.frontier.add(d, n);
            }
        }
    }
}

/// Adds to the frontier the neighbours `around` not yet visited, measured
/// from `x` in one call of `metric` (`Metric::distances`).
fn expand_batched((around, x): (&[u32], &[f32]), (vectors, metric): (Vectors<'_>, &dyn Metric), walk: &mut Walk) -> Result<()> {
    let fresh: Vec<u32> = around.iter().copied().filter(|&n| walk.visited.visit(n)).collect();
    let rows = fresh.iter().map(|&n| vectors.row(row(n))).collect::<Result<Vec<_>>>()?;
    for (n, d) in fresh.into_iter().zip(metric.distances(x, &rows)) {
        walk.frontier.add(finite(d, "metric")?, n);
    }
    Ok(())
}

/// Each node's R random distinct out-neighbours, or every other node if
/// there are no more than R others [Algorithm 3, "a random R-regular
/// directed graph"].
fn random_graph(graph: &mut Graph, random: &mut Random) {
    let n = graph.len();
    let mut list = Vec::with_capacity(graph.degree);
    for id in 0..n {
        list.clear();
        if n.saturating_sub(1) <= graph.degree {
            list.extend((0..n).filter(|&o| o != id).filter_map(|o| u32::try_from(o).ok()));
        } else {
            while list.len() < graph.degree {
                let pick = random.below(n);
                let pick32 = u32::try_from(pick).unwrap_or(0);
                if pick != id && !list.contains(&pick32) {
                    list.push(pick32);
                }
            }
        }
        graph.set(u32::try_from(id).unwrap_or(0), &list);
    }
}

/// The point nearest the centroid of `n` vectors.
pub(super) fn medoid(vectors: Vectors<'_>, n: usize, metric: &dyn Metric) -> Result<u32> {
    let centroid = centroid(vectors.data, vectors.dims);
    let mut best = (f32::INFINITY, 0_u32);
    for at in 0..n {
        let d = finite(metric.distance(&centroid, vectors.row(at)?), "metric")?;
        let id = u32::try_from(at).map_err(|_| corrupt("a build has too many points"))?;
        if d < best.0 {
            best = (d, id);
        }
    }
    Ok(best.1)
}

/// The mean of flat vectors of `dims` coordinates.
pub(super) fn centroid(data: &[f32], dims: usize) -> Vec<f32> {
    let mut sum = vec![0.0_f32; dims];
    let mut count = 0.0_f32;
    for v in data.chunks_exact(dims.max(1)) {
        sum.iter_mut().zip(v).for_each(|(s, x)| *s += x);
        count += 1.0_f32;
    }
    if count > 0.0_f32 {
        for s in &mut sum {
            *s /= count;
        }
    }
    sum
}

/// The row of node `id` in a build's own vectors.
fn row(id: u32) -> usize {
    usize::try_from(id).unwrap_or(usize::MAX)
}
