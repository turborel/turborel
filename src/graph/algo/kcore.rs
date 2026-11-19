//! \[EXTENSION\] K_CORE (the graph standard's §18, `GRAPH_ALGO.K_CORE(g)`):
//! each vertex's core number in the graph's undirected form, the
//! greatest k such that it is in a subgraph whose every vertex has k
//! neighbours in it (Seidman, "Network structure and minimum degree",
//! Social Networks 5, 1983).
//!
//! The cores are peeled level by level (Matula and Beck, "Smallest-last
//! ordering and clustering and graph coloring algorithms", J. ACM 30,
//! 1983, in the rounds of Dhulipala, Blelloch and Shun's Julienne, SPAA
//! 2017): at level k, every vertex left of degree k or less is removed,
//! its core number k; removing it takes one from each neighbour left's
//! degree, and a neighbour whose degree falls to k is removed in the
//! next round of the level. A round's vertices are cut into morsels, the
//! degrees atomic: a neighbour's fall to k is seen by the one decrement
//! that makes it, so it joins the next round once. Core numbers are
//! unique, so every width gives the same.

use std::sync::atomic::{AtomicU32, Ordering};

use super::control::{Cancel, Control};
use super::csr::{Undirected, Vertex, ix, vertex};
use super::parallel::{MORSEL, morsels};
use super::unsettled;
use crate::error::Result;

/// The core number of a vertex not yet removed.
const LEFT: u32 = u32::MAX;

/// The arrays of a vertex held: its degree and its core number.
const ARRAYS: usize = 2;

/// Takes one from `w`'s degree: whether it fell to `k`, which only the
/// decrement from k + 1 sees.
fn falls(degrees: &[AtomicU32], w: Vertex, k: u32) -> bool {
    degrees.get(ix(w)).map(|d| d.fetch_sub(1, Ordering::AcqRel)) == Some(k.saturating_add(1))
}

/// The core number of each vertex of `u`.
///
/// # Errors
///
/// SQLCODE -918 past the budget; the cancel's.
pub fn k_core<C: Cancel>(u: &Undirected<'_>, control: &Control<'_, C>) -> Result<Vec<u32>> {
    let n = u.vertices();
    let _charge = control.memory.items::<u32>(n.saturating_mul(ARRAYS), "K_CORE's degrees")?;
    let degrees: Vec<AtomicU32> = (0..n).map(|v| AtomicU32::new(vertex(u.degree(vertex(v))))).collect();
    let cores: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(LEFT)).collect();
    let load = |of: &[AtomicU32], v: Vertex| of.get(ix(v)).map_or(LEFT, |a| a.load(Ordering::Acquire));
    let mut left = n;
    let mut k = 0_u32;
    while left > 0 {
        // Each level removes at least the vertex of least degree left.
        if ix(k) > n {
            return Err(unsettled("K_CORE's levels"));
        }
        let least = morsels(control, (n, MORSEL), |r| {
            Ok(r.map(vertex).filter(|&v| load(&cores, v) == LEFT).map(|v| load(&degrees, v)).min().unwrap_or(LEFT))
        })?;
        k = k.max(least.into_iter().min().unwrap_or(LEFT));
        let found = morsels(control, (n, MORSEL), |r| {
            Ok(r.map(vertex).filter(|&v| load(&cores, v) == LEFT && load(&degrees, v) <= k).collect::<Vec<_>>())
        })?;
        let mut round = found.concat();
        let mut rounds = left.saturating_add(1);
        while !round.is_empty() {
            // Each round removes at least one vertex left.
            rounds = rounds.checked_sub(1).ok_or_else(|| unsettled("K_CORE's rounds"))?;
            for &v in &round {
                if let Some(c) = cores.get(ix(v)) {
                    c.store(k, Ordering::Release);
                }
            }
            left = left.saturating_sub(round.len());
            let next = morsels(control, (round.len(), MORSEL), |r| {
                let mut next = Vec::new();
                for &v in round.get(r).unwrap_or_default() {
                    next.extend(u.neighbours(v).iter().copied().filter(|&w| load(&cores, w) == LEFT && falls(&degrees, w, k)));
                }
                Ok(next)
            })?;
            round = next.concat();
        }
        k = k.saturating_add(1);
    }
    Ok(cores.into_iter().map(AtomicU32::into_inner).collect())
}
