//! \[EXTENSION\] BFS_LEVELS and SSSP (the graph standard's §18,
//! `GRAPH_ALGO.BFS_LEVELS(g, start)`, `GRAPH_ALGO.SSSP(g, start, cost =>
//! 'w')`): each vertex's number of edges from the start, and its least
//! cost from it, along outgoing edges.
//!
//! BFS_LEVELS is level-synchronous: each level's frontier is cut into
//! morsels, and a vertex not yet reached is claimed for the next level
//! by a compare and swap of its level, so it is reached once whichever
//! thread finds it, at the same level on every width.
//!
//! SSSP of costs none negative is Dijkstra's algorithm ("A note on two
//! problems in connexion with graphs", Numer. Math. 1, 1959), whose
//! vertices are settled one at a time. Of a negative cost it is the
//! Bellman-Ford algorithm (Bellman, "On a routing problem", Q. Appl.
//! Math. 16, 1958), in rounds each of which gives each vertex the least
//! of its cost and its in-neighbours' last costs plus their edges', so a
//! round's morsels are computed alone; a cost that still falls after a
//! round for each vertex is of a negative cycle the start reaches, where
//! no least cost exists (GRF-0034).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicU32, Ordering};

use super::control::{Cancel, Control};
use super::csr::{Graph, Vertex, ix, vertex};
use super::parallel::{MORSEL, morsels};
use super::unsettled;
use crate::error::{Result, SqlError};

/// The level of a vertex not reached.
pub const UNREACHED: u32 = u32::MAX;

/// The level of each vertex of `g` from `start`, `UNREACHED` of one the
/// start does not reach.
///
/// # Errors
///
/// SQLCODE -918 past the budget; the cancel's.
pub fn bfs_levels<C: Cancel>(g: &Graph<'_>, start: Vertex, control: &Control<'_, C>) -> Result<Vec<u32>> {
    let n = g.vertices();
    let _charge = control.memory.items::<u32>(n.saturating_mul(2), "BFS_LEVELS's levels and frontier")?;
    let levels: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(UNREACHED)).collect();
    let Some(first) = levels.get(ix(start)) else { return Ok(vec![UNREACHED; n]) };
    first.store(0, Ordering::Release);
    let mut frontier = vec![start];
    let mut level = 0_u32;
    while !frontier.is_empty() {
        level = level.saturating_add(1);
        // A vertex is reached at one level, so there are fewer levels than
        // vertices.
        if ix(level) > n {
            return Err(unsettled("BFS_LEVELS's levels"));
        }
        let found = morsels(control, (frontier.len(), MORSEL), |r| {
            let mut next = Vec::new();
            for &v in frontier.get(r).unwrap_or_default() {
                next.extend(g.out(v).iter().copied().filter(|&w| claim(&levels, w, level)));
            }
            Ok(next)
        })?;
        frontier = found.concat();
        frontier.sort_unstable();
    }
    Ok(levels.into_iter().map(AtomicU32::into_inner).collect())
}

/// Claims `w` for `level` if no level reached it: whether this did.
fn claim(levels: &[AtomicU32], w: Vertex, level: u32) -> bool {
    levels.get(ix(w)).is_some_and(|l| l.compare_exchange(UNREACHED, level, Ordering::AcqRel, Ordering::Acquire).is_ok())
}

/// A cost and its vertex, ordered by cost, then vertex, for Dijkstra's
/// queue.
#[derive(Debug, Clone, Copy)]
struct Queued(f64, Vertex);

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0).then(self.1.cmp(&other.1))
    }
}

/// The error of a cost not a number [GRF-0034].
fn not_a_number() -> SqlError {
    super::cost_refused("an edge's cost is not a number")
}

/// The least cost from `start` of each vertex of `g`, along its edges'
/// weights (1 each of an unweighted graph), infinite of one the start
/// does not reach.
///
/// # Errors
///
/// SQLCODE -301 if a cost is not a number, or a negative cycle is
/// reachable from the start (GRF-0034); -918 past the budget; the
/// cancel's.
pub fn sssp<C: Cancel>(g: &Graph<'_>, start: Vertex, control: &Control<'_, C>) -> Result<Vec<f64>> {
    let n = g.vertices();
    let _charge = control.memory.items::<f64>(n.saturating_mul(2), "SSSP's costs")?;
    let mut negative = false;
    for v in (0..n).map(vertex) {
        for &w in g.out_weights(v).unwrap_or_default() {
            if w.is_nan() {
                return Err(not_a_number());
            }
            negative = negative || w < 0.0_f64;
        }
    }
    if ix(start) >= n {
        return Ok(vec![f64::INFINITY; n]);
    }
    if negative { bellman_ford(g, start, control) } else { dijkstra(g, start, control) }
}

/// The weight of `v`'s `k`th outgoing edge.
fn weight(g: &Graph<'_>, v: Vertex, k: usize) -> f64 {
    g.out_weights(v).and_then(|w| w.get(k)).copied().unwrap_or(1.0_f64)
}

/// Dijkstra's algorithm from `start`, of costs none negative.
fn dijkstra<C: Cancel>(g: &Graph<'_>, start: Vertex, control: &Control<'_, C>) -> Result<Vec<f64>> {
    let mut costs = vec![f64::INFINITY; g.vertices()];
    let mut settled = vec![false; g.vertices()];
    let mut queue = BinaryHeap::from([Reverse(Queued(0.0_f64, start))]);
    if let Some(c) = costs.get_mut(ix(start)) {
        *c = 0.0_f64;
    }
    let mut settles = 0_usize;
    while let Some(Reverse(Queued(cost, v))) = queue.pop() {
        let Some(done) = settled.get_mut(ix(v)) else { continue };
        if *done {
            continue;
        }
        *done = true;
        settles = settles.saturating_add(1);
        if settles.is_multiple_of(MORSEL) {
            control.check()?;
        }
        for (k, &w) in g.out(v).iter().enumerate() {
            let through = cost + weight(g, v, k);
            let Some(c) = costs.get_mut(ix(w)) else { continue };
            if through < *c {
                *c = through;
                queue.push(Reverse(Queued(through, w)));
            }
        }
    }
    Ok(costs)
}

/// The Bellman-Ford algorithm from `start`, in rounds of morsels.
fn bellman_ford<C: Cancel>(g: &Graph<'_>, start: Vertex, control: &Control<'_, C>) -> Result<Vec<f64>> {
    let n = g.vertices();
    let mut costs = vec![f64::INFINITY; n];
    if let Some(c) = costs.get_mut(ix(start)) {
        *c = 0.0_f64;
    }
    for _ in 0..n {
        let parts = morsels(control, (n, MORSEL), |r| {
            let mut next = Vec::with_capacity(r.len());
            let mut fell = false;
            for v in r.map(vertex) {
                let own = costs.get(ix(v)).copied().unwrap_or(f64::INFINITY);
                let ws = g.in_weights(v).unwrap_or_default();
                let pulled = g.ins(v).iter().zip(ws).map(|(&u, &w)| costs.get(ix(u)).copied().unwrap_or(f64::INFINITY) + w);
                let least = pulled.fold(own, f64::min);
                fell = fell || least < own;
                next.push(least);
            }
            Ok((next, fell))
        })?;
        let fell = parts.iter().any(|(_, f)| *f);
        costs = parts.into_iter().flat_map(|(c, _)| c).collect();
        if !fell {
            return Ok(costs);
        }
    }
    Err(super::cost_refused("the edges' costs make a negative cycle the start reaches, so no least cost exists"))
}
