//! \[DESIGN\] HNSW search (Malkov and Yashunin, §4, Algorithms 2 and 5):
//! a greedy descent through the upper layers to layer 0, then a best-first
//! search keeping the ef nearest. Tombstones and rows a filter refuses are
//! still waypoints, never results, so a selective filter widens the search
//! instead of ending it early (vector design document R9.3). A range
//! search expands the frontier until it passes the radius (R9.1).
//!
//! The graph is read through a `Source`, so the metric and the filter run
//! with no lock of the index held (`view`).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::graph::NONE;
use super::measure::{Metric, Probe};
use super::scratch::{Scored, Scratch, Visited, keep};
use super::view::{Source, Step};

/// Whether a node may be a result, by its id and its row if it is live.
pub type Accept<'a> = dyn FnMut(u32, Option<u64>) -> bool + 'a;

/// The node nearest `q` reached greedily on `level` from `from`. Each
/// step is strictly nearer, so the walk ends. A node measured before is
/// not measured again: it was no nearer than the best then, and the best
/// only nears.
fn closest<S: Source>(src: &mut S, (metric, q): (&Metric, Probe<'_>), from: Scored, (level, visited): (u8, &mut Visited)) -> Step<Scored> {
    let mut best = from;
    loop {
        let at = best;
        src.hop((at.id, level), visited, &mut |e, _, v| {
            let near = Scored { distance: metric.between(q, v), id: e };
            if near < best {
                best = near;
            }
        })?;
        if best.id == at.id {
            return Ok(best);
        }
    }
}

/// The node nearest `q` on layer `level`, descending from `entry` on
/// layer `top`; none if the graph is empty.
///
/// # Errors
///
/// The source's.
pub fn descend<S: Source>(
    src: &mut S,
    (metric, q): (&Metric, Probe<'_>),
    (entry, top, level): (u32, u8, u8),
    s: &mut Scratch,
) -> Step<Option<Scored>> {
    if entry == NONE {
        return Ok(None);
    }
    src.node(entry, &mut s.a)?;
    let mut best = Scored { distance: metric.distance(q.0, &s.a), id: entry };
    s.visited.begin(0);
    s.visited.first(entry);
    let mut at = top;
    while at > level {
        best = closest(src, (metric, q), best, (at, &mut s.visited))?;
        at = at.saturating_sub(1);
    }
    Ok(Some(best))
}

/// SEARCH-LAYER (Algorithm 2): the `ef` nearest nodes `accept` takes,
/// on `level`, from `entries`, left in `s.results`. The visited set is
/// begun by the caller.
///
/// # Errors
///
/// The source's.
pub fn search_layer<S: Source>(
    src: &mut S,
    (metric, q): (&Metric, Probe<'_>),
    (entries, ef, level): (&[Scored], usize, u8),
    s: &mut Scratch,
    accept: &mut Accept<'_>,
) -> Step<()> {
    let Scratch { visited, candidates, results, .. } = s;
    results.clear();
    candidates.clear();
    for e in entries {
        if visited.first(e.id) {
            candidates.push(Reverse(*e));
            if accept(e.id, src.row(e.id)?) {
                keep(results, *e, ef);
            }
        }
    }
    while let Some(Reverse(c)) = candidates.pop() {
        if results.len() >= ef && results.peek().is_some_and(|far| c > *far) {
            break;
        }
        src.hop((c.id, level), visited, &mut |e, row, v| {
            let near = Scored { distance: metric.between(q, v), id: e };
            if results.len() >= ef && results.peek().is_some_and(|far| near >= *far) {
                return;
            }
            candidates.push(Reverse(near));
            if accept(e, row) {
                keep(results, near, ef);
            }
        })?;
    }
    Ok(())
}

/// K-NN-SEARCH (Algorithm 5): the `k` nearest nodes `accept` takes,
/// searching with `ef`, nearest first, in `s.chosen`.
///
/// # Errors
///
/// The source's.
pub fn knn<S: Source>(
    src: &mut S,
    (metric, q): (&Metric, Probe<'_>),
    (k, ef): (usize, usize),
    s: &mut Scratch,
    accept: &mut Accept<'_>,
) -> Step<()> {
    s.chosen.clear();
    let (entry, top) = src.entry()?;
    let Some(start) = descend(src, (metric, q), (entry, top, 0), s)? else { return Ok(()) };
    s.visited.begin(0);
    search_layer(src, (metric, q), (&[start], ef.max(k), 0), s, accept)?;
    s.chosen.extend(s.results.drain());
    s.chosen.sort_unstable();
    s.chosen.truncate(k);
    Ok(())
}

/// Every node `accept` takes within `radius` of `q`, nearest first, in
/// `s.chosen`: a best-first expansion from layer 0's entry, which
/// stops once its nearest unexpanded node is past the radius, after at
/// least `ef` expansions to reach the radius's region. Each candidate
/// carries its row, read with it, so taking it reads the graph no more.
///
/// # Errors
///
/// The source's.
pub fn range<S: Source>(
    src: &mut S,
    (metric, q): (&Metric, Probe<'_>),
    (radius, ef): (f32, usize),
    s: &mut Scratch,
    accept: &mut Accept<'_>,
) -> Step<()> {
    s.chosen.clear();
    let (entry, top) = src.entry()?;
    let Some(start) = descend(src, (metric, q), (entry, top, 0), s)? else { return Ok(()) };
    let Scratch { visited, chosen, .. } = s;
    visited.begin(0);
    visited.first(start.id);
    let mut candidates = BinaryHeap::new();
    candidates.push(Reverse((start, src.row(start.id)?)));
    let mut expanded = 0_usize;
    while let Some(Reverse((c, row))) = candidates.pop() {
        if c.distance > radius && expanded >= ef {
            break;
        }
        expanded = expanded.saturating_add(1);
        if c.distance <= radius && accept(c.id, row) {
            chosen.push(c);
        }
        src.hop((c.id, 0), visited, &mut |e, live, v| candidates.push(Reverse((Scored { distance: metric.between(q, v), id: e }, live))))?;
    }
    chosen.sort_unstable();
    Ok(())
}
