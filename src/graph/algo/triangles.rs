//! \[EXTENSION\] TRIANGLE_COUNT and LOCAL_CLUSTERING (the graph standard's
//! §18, `GRAPH_ALGO.TRIANGLE_COUNT(g)`, `GRAPH_ALGO.LOCAL_CLUSTERING(g)`),
//! of the graph's undirected form: the triangles each vertex is a corner
//! of, and the share of its neighbours' pairs that are neighbours too.
//!
//! The count is the oriented one of Schank and Wagner ("Finding,
//! counting and listing all triangles in large graphs", WEA 2005; the
//! "forward" algorithm): each edge is kept once, from the endpoint of
//! lower degree (of lower number between equals) to the other, so each
//! vertex keeps at most the square root of twice the edges, and each
//! triangle is found once, at its first corner, by intersecting two
//! such sorted lists. A morsel of first corners is counted alone, each
//! triangle added to its three corners' atomic counts, which are exact
//! whatever order they come in.

use std::sync::atomic::{AtomicU64, Ordering};

use super::control::{Cancel, Control};
use super::csr::{Undirected, Vertex, ix, real, vertex};
use super::parallel::{MORSEL, morsels};
use crate::error::Result;

/// The orders of a pair: d(d - 1) counts each pair of d neighbours in
/// both.
const ORDERS: f64 = 2.0_f64;

/// The ends of an edge: each is in two vertices' neighbours.
const ENDS: usize = 2;

/// The triangles each vertex of `u` is a corner of.
///
/// # Errors
///
/// SQLCODE -918 past the budget; the cancel's.
pub fn triangle_count<C: Cancel>(u: &Undirected<'_>, control: &Control<'_, C>) -> Result<Vec<u64>> {
    let n = u.vertices();
    let first = |a: Vertex, b: Vertex| (u.degree(a), a) < (u.degree(b), b);
    let kept = u.entries().div_ceil(ENDS).saturating_mul(size_of::<Vertex>()).saturating_add(n.saturating_mul(size_of::<Vec<Vertex>>()));
    let _lists = control.memory.charge(kept, "TRIANGLE_COUNT's oriented edges")?;
    let parts = morsels(control, (n, MORSEL), |r| {
        let mut lists = Vec::with_capacity(r.len());
        for v in r.map(vertex) {
            lists.push(u.neighbours(v).iter().copied().filter(|&w| first(v, w)).collect::<Vec<Vertex>>());
        }
        Ok(lists)
    })?;
    let forward: Vec<Vec<Vertex>> = parts.into_iter().flatten().collect();
    let _charge = control.memory.items::<u64>(n, "TRIANGLE_COUNT's counts")?;
    let counts: Vec<AtomicU64> = (0..n).map(|_| AtomicU64::new(0)).collect();
    let add = |v: Vertex| {
        if let Some(c) = counts.get(ix(v)) {
            c.fetch_add(1, Ordering::Relaxed);
        }
    };
    morsels(control, (n, MORSEL), |r| {
        for v in r.map(vertex) {
            let mine = forward.get(ix(v)).map(Vec::as_slice).unwrap_or_default();
            for &w in mine {
                let theirs = forward.get(ix(w)).map(Vec::as_slice).unwrap_or_default();
                for x in common(mine, theirs) {
                    add(v);
                    add(w);
                    add(x);
                }
            }
        }
        Ok(())
    })?;
    Ok(counts.into_iter().map(AtomicU64::into_inner).collect())
}

/// The vertices of both sorted lists `a` and `b`, in order.
pub(crate) fn common<'a>(first: &'a [Vertex], second: &'a [Vertex]) -> impl Iterator<Item = Vertex> + 'a {
    let (mut left, mut right) = (first.iter().peekable(), second.iter().peekable());
    std::iter::from_fn(move || {
        loop {
            let (&&x, &&y) = (left.peek()?, right.peek()?);
            match x.cmp(&y) {
                std::cmp::Ordering::Less => {
                    left.next();
                }
                std::cmp::Ordering::Greater => {
                    right.next();
                }
                std::cmp::Ordering::Equal => {
                    left.next();
                    right.next();
                    return Some(x);
                }
            }
        }
    })
}

/// The local clustering coefficient of each vertex of `u`, of its
/// `triangles`: its triangles over the pairs of its neighbours, 0 of a
/// vertex of fewer than two.
pub fn local_clustering(u: &Undirected<'_>, triangles: &[u64]) -> Vec<f64> {
    (0..u.vertices())
        .map(|v| {
            let d = real(u.degree(vertex(v)));
            let t = triangles.get(v).map_or(0.0_f64, |&t| real(usize::try_from(t).unwrap_or(usize::MAX)));
            let pairs = d * (d - 1.0_f64);
            if pairs > 0.0_f64 { ORDERS * t / pairs } else { 0.0_f64 }
        })
        .collect()
}
