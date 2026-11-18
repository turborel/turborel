//! \[EXTENSION\] WCC (the graph standard's §18, `GRAPH_ALGO.WCC(g)`): the
//! weakly connected components, those of the graph with its edges'
//! directions forgotten, by union-find. Each vertex's parent is a vertex
//! of lower number, or itself if it is a root, and a union links the
//! greater root under the lesser, so each component's root is its least
//! vertex: the label every vertex of it gets.
//!
//! The parents are atomic, so a run's threads take morsels of vertices
//! and join the ends of their edges at once (the lock-free union-find of
//! Anderson and Woll, "Wait-free parallel algorithms for the union-find
//! problem", STOC 1991, of links by number): a link is a compare and
//! swap of a root's parent, retried once another link took the root
//! first. Its roots, and so its labels, are the same on any width.

use std::sync::atomic::{AtomicU32, Ordering};

use super::control::{Cancel, Control};
use super::csr::{Graph, Vertex, ix, vertex};
use super::parallel::{MORSEL, morsels};
use crate::error::Result;

/// The weakly connected component of each vertex of `g`, by its least
/// vertex.
///
/// # Errors
///
/// SQLCODE -918 past the budget; the cancel's.
pub fn wcc<C: Cancel>(g: &Graph<'_>, control: &Control<'_, C>) -> Result<Vec<Vertex>> {
    let n = g.vertices();
    let _charge = control.memory.items::<AtomicU32>(n.saturating_mul(2), "WCC's parents")?;
    let parents: Vec<AtomicU32> = (0..n).map(|v| AtomicU32::new(vertex(v))).collect();
    morsels(control, (n, MORSEL), |r| {
        for v in r.map(vertex) {
            for &u in g.out(v) {
                union(&parents, v, u);
            }
        }
        Ok(())
    })?;
    Ok(morsels(control, (n, MORSEL), |r| Ok(r.map(|v| find(&parents, vertex(v))).collect::<Vec<_>>()))?.concat())
}

/// The parent of `x`.
fn parent(parents: &[AtomicU32], x: Vertex) -> Vertex {
    parents.get(ix(x)).map_or(x, |p| p.load(Ordering::Acquire))
}

/// The root of `x`, its path halved on the way: each vertex passed
/// points to its grandparent, if no link changed its parent meanwhile.
fn find(parents: &[AtomicU32], x: Vertex) -> Vertex {
    let mut x = x;
    // Each step goes to a lower number, so a root is at most as many steps
    // away as there are vertices.
    for _ in 0..=parents.len() {
        let p = parent(parents, x);
        if p == x {
            return x;
        }
        let grand = parent(parents, p);
        if let Some(own) = parents.get(ix(x)) {
            // A failed swap means another thread moved x's parent on,
            // closer to the root: either is a correct parent.
            let _ = own.compare_exchange(p, grand, Ordering::AcqRel, Ordering::Acquire);
        }
        x = grand;
    }
    x
}

/// Joins the components of `a` and `b`: the greater root linked under
/// the lesser, retried if another link took the greater first.
fn union(parents: &[AtomicU32], a: Vertex, b: Vertex) {
    // A retry follows another link, which leaves a root fewer.
    for _ in 0..=parents.len() {
        let (ra, rb) = (find(parents, a), find(parents, b));
        if ra == rb {
            return;
        }
        let (low, high) = (ra.min(rb), ra.max(rb));
        let Some(slot) = parents.get(ix(high)) else { return };
        if slot.compare_exchange(high, low, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return;
        }
    }
}
