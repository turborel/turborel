//! \[EXTENSION\] SCC (the graph standard's §18, `GRAPH_ALGO.SCC(g)`): the
//! strongly connected components, each vertex labelled by its
//! component's least vertex.
//!
//! On one thread it is Tarjan's algorithm ("Depth-first search and
//! linear graph algorithms", SIAM J. Comput. 1, 1972), its depth-first
//! search kept on a stack of frames of its own rather than the call
//! stack, so no graph is too deep for it (RULES.md R-CF-6).
//!
//! On more it is the colouring algorithm (Orzan, "On distributed
//! verification and verified distribution", 2004, §4; Slota, Rajamanickam
//! and Madduri, "BFS and coloring-based parallel algorithms for strongly
//! connected components", IPDPS 2014): the vertices of no edge in or out
//! among those left are trimmed, each its own component; then each vertex
//! left takes the least number of the vertices that reach it, spread
//! along edges until nothing changes, and each vertex whose colour is its
//! own is the least of its component, which is the vertices of its colour
//! that reach it, found by a search back along edges. Those are taken out
//! and the rest coloured again. Each component's label is its least
//! vertex either way, so both give the same labels.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::control::{Cancel, Control};
use super::csr::{Graph, Vertex, ix, vertex};
use super::parallel::{MORSEL, morsels};
use super::unsettled;
use crate::error::Result;

/// A vertex not yet numbered, or not yet labelled.
const UNSET: Vertex = Vertex::MAX;

/// The arrays of a vertex's numbers Tarjan's algorithm holds: its
/// number, its low link, its label, and its places on the two stacks.
const TARJAN_ARRAYS: usize = 5;

/// The strongly connected component of each vertex of `g`, by its least
/// vertex.
///
/// # Errors
///
/// SQLCODE -918 past the budget; the cancel's.
pub fn scc<C: Cancel>(g: &Graph<'_>, control: &Control<'_, C>) -> Result<Vec<Vertex>> {
    if control.threads() <= 1 { tarjan(g, control) } else { coloured(g, control) }
}

/// The state of Tarjan's search.
struct Tarjan {
    index: Vec<Vertex>,
    low: Vec<Vertex>,
    on_stack: Vec<bool>,
    stack: Vec<Vertex>,
    labels: Vec<Vertex>,
    next: Vertex,
}

impl Tarjan {
    /// Numbers `v` and puts it on the stack.
    fn visit(&mut self, v: Vertex) {
        put(&mut self.index, v, self.next);
        put(&mut self.low, v, self.next);
        self.next = self.next.saturating_add(1);
        self.stack.push(v);
        if let Some(on) = self.on_stack.get_mut(ix(v)) {
            *on = true;
        }
    }

    /// Lowers `v`'s low link to `to`, if lower.
    fn lower(&mut self, v: Vertex, to: Vertex) {
        if let Some(l) = self.low.get_mut(ix(v)) {
            *l = (*l).min(to);
        }
    }

    /// Takes `v`'s component off the stack, if `v` is its root.
    fn close(&mut self, v: Vertex) {
        if at(&self.low, v) != at(&self.index, v) {
            return;
        }
        let mut members = Vec::new();
        while let Some(w) = self.stack.pop() {
            if let Some(on) = self.on_stack.get_mut(ix(w)) {
                *on = false;
            }
            members.push(w);
            if w == v {
                break;
            }
        }
        let least = members.iter().copied().min().unwrap_or(v);
        for w in members {
            put(&mut self.labels, w, least);
        }
    }
}

/// The value at `v` of `of`, `UNSET` past it.
fn at(of: &[Vertex], v: Vertex) -> Vertex {
    of.get(ix(v)).copied().unwrap_or(UNSET)
}

/// Sets the value at `v` of `of`.
fn put(of: &mut [Vertex], v: Vertex, value: Vertex) {
    if let Some(x) = of.get_mut(ix(v)) {
        *x = value;
    }
}

/// Tarjan's algorithm on `g`, its search's frames each a vertex and the
/// place of its next edge.
fn tarjan<C: Cancel>(g: &Graph<'_>, control: &Control<'_, C>) -> Result<Vec<Vertex>> {
    let count = g.vertices();
    let _charge = control.memory.items::<Vertex>(count.saturating_mul(TARJAN_ARRAYS), "SCC's search")?;
    let mut t = Tarjan {
        index: vec![UNSET; count],
        low: vec![UNSET; count],
        on_stack: vec![false; count],
        stack: Vec::new(),
        labels: vec![UNSET; count],
        next: 0,
    };
    let mut frames: Vec<(Vertex, usize)> = Vec::new();
    for s in (0..count).map(vertex) {
        if at(&t.index, s) != UNSET {
            continue;
        }
        control.check()?;
        t.visit(s);
        frames.push((s, 0));
        while let Some((v, edge)) = frames.last_mut() {
            let v = *v;
            let Some(&w) = g.out(v).get(*edge) else {
                frames.pop();
                if let Some(&(parent, _)) = frames.last() {
                    t.lower(parent, at(&t.low, v));
                }
                t.close(v);
                continue;
            };
            *edge = edge.saturating_add(1);
            if at(&t.index, w) == UNSET {
                t.visit(w);
                frames.push((w, 0));
            } else if t.on_stack.get(ix(w)).copied().unwrap_or(false) {
                t.lower(v, at(&t.index, w));
            }
        }
    }
    Ok(t.labels)
}

/// The colouring algorithm on `g`.
fn coloured<C: Cancel>(g: &Graph<'_>, control: &Control<'_, C>) -> Result<Vec<Vertex>> {
    let n = g.vertices();
    let _charge = control.memory.items::<Vertex>(n.saturating_mul(TARJAN_ARRAYS), "SCC's colours")?;
    let labels: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(UNSET)).collect();
    let colours: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(UNSET)).collect();
    // Each round labels at least the least vertex left.
    for _ in 0..=n {
        trim(g, &labels);
        let left: Vec<Vertex> = (0..n).map(vertex).filter(|&v| unlabelled(&labels, v)).collect();
        if left.is_empty() {
            break;
        }
        colour(g, (&left, &labels, &colours), control)?;
        let roots: Vec<Vertex> = left.iter().copied().filter(|&v| load(&colours, v) == v).collect();
        let found = morsels(control, (roots.len(), 1), |r| {
            Ok(roots.get(r).unwrap_or_default().iter().map(|&root| (root, back(g, root, &labels, &colours))).collect::<Vec<_>>())
        })?;
        for (root, members) in found.into_iter().flatten() {
            for v in members {
                store(&labels, v, root);
            }
        }
    }
    if (0..n).map(vertex).any(|v| unlabelled(&labels, v)) {
        return Err(unsettled("SCC's colouring"));
    }
    Ok(labels.into_iter().map(AtomicU32::into_inner).collect())
}

/// The value at `v` of `of`.
fn load(of: &[AtomicU32], v: Vertex) -> Vertex {
    of.get(ix(v)).map_or(UNSET, |a| a.load(Ordering::Acquire))
}

/// Sets the value at `v` of `of`.
fn store(of: &[AtomicU32], v: Vertex, value: Vertex) {
    if let Some(a) = of.get(ix(v)) {
        a.store(value, Ordering::Release);
    }
}

/// Whether `v` is not yet of a component.
fn unlabelled(labels: &[AtomicU32], v: Vertex) -> bool {
    load(labels, v) == UNSET
}

/// Labels each vertex left of no edge from or to a vertex left but
/// itself as its own component, again until none is: none such is on a
/// cycle (the trimming of McLendon and others, "Finding strongly
/// connected components in distributed graphs", JPDC 65, 2005).
fn trim(g: &Graph<'_>, labels: &[AtomicU32]) {
    let live = |v: Vertex, ends: &[Vertex]| ends.iter().any(|&u| u != v && unlabelled(labels, u));
    let mut queue: Vec<Vertex> = (0..g.vertices()).map(vertex).collect();
    while let Some(v) = queue.pop() {
        if !unlabelled(labels, v) || (live(v, g.out(v)) && live(v, g.ins(v))) {
            continue;
        }
        store(labels, v, v);
        queue.extend(g.out(v).iter().chain(g.ins(v)).copied().filter(|&u| unlabelled(labels, u)));
    }
}

/// Gives each vertex of `left` the least vertex of `left` that reaches
/// it: its own number, then the least of its in-neighbours' colours,
/// spread until no colour changes.
fn colour<C: Cancel>(
    g: &Graph<'_>,
    (left, labels, colours): (&[Vertex], &[AtomicU32], &[AtomicU32]),
    control: &Control<'_, C>,
) -> Result<()> {
    for &v in left {
        store(colours, v, v);
    }
    let changed = AtomicBool::new(true);
    // A colour moves one edge a round at least, so settles within as
    // many rounds as vertices.
    let mut rounds = left.len().saturating_add(1);
    while changed.load(Ordering::Acquire) {
        rounds = rounds.checked_sub(1).ok_or_else(|| unsettled("SCC's colours"))?;
        changed.store(false, Ordering::Release);
        morsels(control, (left.len(), MORSEL), |r| {
            for &v in left.get(r).unwrap_or_default() {
                let mine = load(colours, v);
                let lowered = g.out(v).iter().filter(|&&w| unlabelled(labels, w) && lower(colours, w, mine)).count();
                if lowered > 0 {
                    changed.store(true, Ordering::Release);
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// Lowers `w`'s colour to `to`: whether it was higher.
fn lower(colours: &[AtomicU32], w: Vertex, to: Vertex) -> bool {
    colours.get(ix(w)).is_some_and(|c| c.fetch_min(to, Ordering::AcqRel) > to)
}

/// The vertices of `root`'s colour that reach it: its component.
fn back(g: &Graph<'_>, root: Vertex, labels: &[AtomicU32], colours: &[AtomicU32]) -> Vec<Vertex> {
    let mut members = vec![root];
    let mut seen = std::collections::HashSet::from([root]);
    let mut k = 0;
    while let Some(&v) = members.get(k) {
        k = k.saturating_add(1);
        for &u in g.ins(v) {
            if load(colours, u) == root && unlabelled(labels, u) && !seen.contains(&u) {
                seen.insert(u);
                members.push(u);
            }
        }
    }
    members
}
