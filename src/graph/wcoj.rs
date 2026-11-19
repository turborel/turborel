//! \[EXTENSION\] [GS §16 R16.2] A worst-case optimal join (RULES.md D114):
//! the matches of rigid path patterns whose node patterns make a cycle,
//! by generic join over the adjacency.
//!
//! The vertex variables are bound one at a time, each next the one most
//! edge patterns tie to those bound. Its candidates are the intersection
//! of the sorted neighbour lists of the vertices it is tied to, so a
//! triangle reads no pair of edges that does not close. A component's
//! first vertex is one of its path's starts. The edges are bound last,
//! those between each pair of bound vertices, then each path's binding is
//! made and kept once, as a search keeps it [P1 §6.5]: the paths' matches
//! are joined on their shared variables as searched ones are.

use std::collections::BTreeSet;

use super::binding::PathBinding;
use super::program::{Bound, ElementTest, PathPattern, Pattern, Restrictor, VarId};
use super::view::{ElementId, GraphView, Hop, Orientation};
use crate::error::Result;

/// One element pattern of a path, and the variable it binds, by its
/// number across the paths.
struct Place<'a> {
    test: &'a ElementTest,
    node: usize,
    /// Of an edge pattern, its orientation.
    orientation: Option<Orientation>,
}

/// An edge pattern between two node patterns, by their numbers.
#[derive(Clone, Copy)]
struct Tie {
    before: usize,
    edge: usize,
    after: usize,
    orientation: Orientation,
}

/// Rigid path patterns, as generic join reads them.
pub struct Rigid<'a> {
    patterns: &'a [PathPattern],
    paths: Vec<Vec<Place<'a>>>,
    ties: Vec<Tie>,
    /// Of each variable, by its number, whether it binds edges.
    edges: Vec<bool>,
    /// Each path's variables' numbers, by their number in it.
    of_path: Vec<Vec<(VarId, usize)>>,
}

impl<'a> Rigid<'a> {
    /// `patterns`, if each is a concatenation of element patterns from a
    /// node to a node; `global` numbers a path's variable across them,
    /// below `globals`, and an element pattern of none is numbered after.
    pub fn new(patterns: &'a [PathPattern], (global, globals): (&dyn Fn(usize, VarId) -> Option<usize>, usize)) -> Option<Rigid<'a>> {
        let mut r = Rigid { patterns, paths: Vec::new(), ties: Vec::new(), edges: vec![false; globals], of_path: Vec::new() };
        for (n, pattern) in patterns.iter().enumerate() {
            let mut places = Vec::new();
            flatten(&pattern.pattern, &mut places)?;
            let mut path = Vec::new();
            let mut own = Vec::new();
            for (test, orientation) in places {
                let node = test.var.and_then(|v| global(n, v)).filter(|&g| g < globals).unwrap_or(r.edges.len());
                if r.edges.len() == node {
                    r.edges.push(false);
                }
                *r.edges.get_mut(node)? = orientation.is_some();
                own.extend(test.var.map(|v| (v, node)));
                path.push(Place { test, node, orientation });
            }
            r.ties.extend(ties(&path)?);
            r.paths.push(path);
            r.of_path.push(own);
        }
        Some(r)
    }

    /// The bindings of each path, from its starts `starts`, in `view`.
    ///
    /// # Errors
    ///
    /// Any error reading the graph or of a prefilter or postfilter.
    pub fn bindings(&self, view: &dyn GraphView, starts: &[Vec<ElementId>]) -> Result<Vec<Vec<PathBinding>>> {
        let order = self.order();
        let mut found: Vec<BTreeSet<Vec<ElementId>>> = self.paths.iter().map(|_| BTreeSet::new()).collect();
        let mut bound = vec![None; self.edges.len()];
        let mut run = Run { rigid: self, view, starts, order: &order, found: &mut found };
        run.vertices(0, &mut bound)?;
        let mut out = Vec::new();
        for (n, kept) in found.into_iter().enumerate() {
            let mut bindings = Vec::new();
            for elements in kept {
                let binding = self.binding(n, elements);
                if self.kept(n, &binding)? {
                    bindings.push(binding);
                }
            }
            out.push(bindings);
        }
        Ok(out)
    }

    /// The order the vertex variables are bound in, each with the path it
    /// starts, if no variable bound before is tied to it.
    fn order(&self) -> Vec<(usize, Option<usize>)> {
        let mut order: Vec<(usize, Option<usize>)> = Vec::new();
        let placed = |so_far: &[(usize, Option<usize>)], node: usize| so_far.iter().any(|&(o, _)| o == node);
        loop {
            let tied = |node: usize| self.ties.iter().filter(|t| tie_of(t, node, &|o| placed(&order, o)).is_some()).count();
            let open = self.paths.iter().flatten().filter(|p| p.orientation.is_none() && !placed(&order, p.node));
            let next = open.map(|p| (tied(p.node), p.node)).filter(|&(n, _)| n > 0).max_by_key(|&(n, _)| n);
            if let Some((_, node)) = next {
                order.push((node, None));
                continue;
            }
            let fresh =
                self.paths.iter().enumerate().find_map(|(n, path)| path.first().filter(|p| !placed(&order, p.node)).map(|p| (p.node, n)));
            match fresh {
                Some((node, n)) => order.push((node, Some(n))),
                None => return order,
            }
        }
    }

    /// The binding of path `n` of `elements`, its variables in place.
    fn binding(&self, n: usize, elements: Vec<ElementId>) -> PathBinding {
        let places = self.paths.get(n).map_or(&[][..], Vec::as_slice);
        let vars = places.iter().map(|p| p.test.var.into_iter().collect()).collect();
        let tags = places.iter().filter(|p| p.orientation.is_none()).map(|_| Vec::new()).collect();
        PathBinding { elements, vars, tags, cost: None }
    }

    /// Whether path `n`'s restrictor and postfilter keep `binding`.
    fn kept(&self, n: usize, binding: &PathBinding) -> Result<bool> {
        let Some(pattern) = self.patterns.get(n) else { return Ok(false) };
        let restricted = match pattern.restrictor {
            Restrictor::Walk => true,
            Restrictor::Trail => binding.is_trail(),
            Restrictor::Acyclic => binding.is_acyclic(),
            Restrictor::Simple => binding.is_simple(),
        };
        match &pattern.postfilter {
            Some(hook) if restricted => (hook.0)(binding),
            Some(_) | None => Ok(restricted),
        }
    }
}

/// The element patterns of `pattern` in order, if it is a concatenation
/// of them alone.
fn flatten<'a>(pattern: &'a Pattern, out: &mut Vec<(&'a ElementTest, Option<Orientation>)>) -> Option<()> {
    match pattern {
        Pattern::Vertex(t) => out.push((t, None)),
        Pattern::Edge(t, o) => out.push((t, Some(*o))),
        Pattern::Concat(parts) => parts.iter().try_for_each(|p| flatten(p, out))?,
        Pattern::Union(_) | Pattern::Alternation(_) | Pattern::Quantified(_) | Pattern::Group(_) => return None,
    }
    Some(())
}

/// The edge patterns of `path`, if its node and edge patterns alternate
/// from a node to a node.
fn ties(path: &[Place<'_>]) -> Option<Vec<Tie>> {
    let alternate = path.iter().enumerate().all(|(i, p)| p.orientation.is_some() == (i % 2 == 1));
    if !alternate || path.len().is_multiple_of(2) {
        return None;
    }
    let edges = path.iter().enumerate().filter_map(|(i, p)| p.orientation.map(|o| (i, p.node, o)));
    edges
        .map(|(i, edge, orientation)| {
            let before = path.get(i.checked_sub(1)?)?.node;
            let after = path.get(i.checked_add(1)?)?.node;
            Some(Tie { before, edge, after, orientation })
        })
        .collect()
}

/// Of tie `t` and its node `node`, the other node, if `bound` holds it,
/// and the orientation of walking from it to `node`; none of a self-loop.
fn tie_of(t: &Tie, node: usize, bound: &dyn Fn(usize) -> bool) -> Option<(usize, Orientation)> {
    if t.before == t.after {
        return None;
    }
    if t.after == node && bound(t.before) {
        return Some((t.before, t.orientation));
    }
    (t.before == node && bound(t.after)).then(|| (t.after, t.orientation.reversed()))
}

/// One generic join's state.
struct Run<'r, 'a> {
    rigid: &'r Rigid<'a>,
    view: &'r dyn GraphView,
    starts: &'r [Vec<ElementId>],
    order: &'r [(usize, Option<usize>)],
    found: &'r mut Vec<BTreeSet<Vec<ElementId>>>,
}

impl Run<'_, '_> {
    /// Binds the vertex variables from the `depth`th of the order on.
    fn vertices(&mut self, depth: usize, bound: &mut Vec<Option<ElementId>>) -> Result<()> {
        let Some(&(node, start)) = self.order.get(depth) else { return self.edges(0, bound) };
        let candidates = match start {
            Some(n) => self.starts.get(n).cloned().unwrap_or_default(),
            None => self.intersected(node, bound)?,
        };
        for vertex in candidates {
            if !self.holds(node, vertex, bound)? {
                continue;
            }
            set(bound, node, Some(vertex));
            self.vertices(depth.saturating_add(1), bound)?;
        }
        set(bound, node, None);
        Ok(())
    }

    /// The vertices every tie of `node` to a bound vertex reaches,
    /// sorted: the intersection of their neighbour lists.
    fn intersected(&self, node: usize, bound: &[Option<ElementId>]) -> Result<Vec<ElementId>> {
        let mut out: Option<Vec<ElementId>> = None;
        let mut hops = Vec::new();
        for t in &self.rigid.ties {
            let Some((other, orientation)) = tie_of(t, node, &|o| bound.get(o).copied().flatten().is_some()) else { continue };
            let Some(from) = bound.get(other).copied().flatten() else { continue };
            hops.clear();
            self.view.hops(from, orientation, &mut hops)?;
            let mut reached: Vec<ElementId> = hops.iter().map(|h| h.vertex).collect();
            reached.sort_unstable();
            reached.dedup();
            out = Some(match out {
                Some(so_far) => intersect(&so_far, &reached),
                None => reached,
            });
        }
        Ok(out.unwrap_or_default())
    }

    /// Binds the edge patterns from the `at`th on, between bound vertices.
    fn edges(&mut self, at: usize, bound: &mut Vec<Option<ElementId>>) -> Result<()> {
        let Some(&t) = self.rigid.ties.get(at) else { return self.found(bound) };
        let (Some(from), Some(to)) = (bound.get(t.before).copied().flatten(), bound.get(t.after).copied().flatten()) else { return Ok(()) };
        let mut hops: Vec<Hop> = Vec::new();
        self.view.hops(from, t.orientation, &mut hops)?;
        let mut edges: Vec<ElementId> = hops.iter().filter(|h| h.vertex == to).map(|h| h.edge).collect();
        edges.sort_unstable();
        edges.dedup();
        let before = bound.get(t.edge).copied().flatten();
        for edge in edges {
            if before.is_some_and(|b| b != edge) || !self.holds(t.edge, edge, bound)? {
                continue;
            }
            set(bound, t.edge, Some(edge));
            self.edges(at.saturating_add(1), bound)?;
            set(bound, t.edge, before);
        }
        Ok(())
    }

    /// Keeps each path's elements of the whole binding `bound`, if every
    /// prefilter reading other variables holds of it too.
    fn found(&mut self, bound: &[Option<ElementId>]) -> Result<()> {
        let mut made = Vec::new();
        for (n, path) in self.rigid.paths.iter().enumerate() {
            let mut elements = Vec::new();
            for place in path {
                let Some(element) = bound.get(place.node).copied().flatten() else { return Ok(()) };
                let reading = place.test.filter.as_ref().is_some_and(|f| !f.reads.is_empty());
                if reading && !self.filtered(n, place, element, bound)? {
                    return Ok(());
                }
                elements.push(element);
            }
            made.push(elements);
        }
        for (kept, elements) in self.found.iter_mut().zip(made) {
            kept.insert(elements);
        }
        Ok(())
    }

    /// Whether `element` passes the label tests of each place of `node`,
    /// and the prefilters that read no other variable.
    fn holds(&self, node: usize, element: ElementId, bound: &[Option<ElementId>]) -> Result<bool> {
        let mut labels = Vec::new();
        self.view.labels(element, &mut labels)?;
        for (n, path) in self.rigid.paths.iter().enumerate() {
            for place in path.iter().filter(|p| p.node == node) {
                let alone = place.test.filter.as_ref().is_none_or(|f| f.reads.is_empty());
                if !place.test.labels.as_ref().is_none_or(|l| l.holds(&labels)) || (alone && !self.filtered(n, place, element, bound)?) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Whether the prefilter of `place` of path `n` holds of `element`,
    /// the variables it reads bound as `bound` binds them.
    fn filtered(&self, n: usize, place: &Place<'_>, element: ElementId, bound: &[Option<ElementId>]) -> Result<bool> {
        let Some(filter) = &place.test.filter else { return Ok(true) };
        let of_path = self.rigid.of_path.get(n).map_or(&[][..], Vec::as_slice);
        let node_of = |var: VarId| of_path.iter().find(|(v, _)| *v == var).map(|&(_, node)| node);
        let registers: Vec<Box<[ElementId]>> =
            filter.reads.iter().map(|r| node_of(r.var).and_then(|k| bound.get(k).copied().flatten()).into_iter().collect()).collect();
        // A rigid pattern has no quantifier, so PREV and FIRST read nothing.
        let reads: Vec<crate::graph::program::Resolved> =
            filter.reads.iter().enumerate().map(|(i, r)| (r.var, (!r.previous && !r.first).then_some(i), None, None)).collect();
        (filter.test)(&Bound { element: Some((place.test.var, element)), registers: &registers, reads: &reads, path: &[] })
    }
}

/// Sets `bound`'s `node` to `to`.
fn set(bound: &mut [Option<ElementId>], node: usize, to: Option<ElementId>) {
    if let Some(slot) = bound.get_mut(node) {
        *slot = to;
    }
}

/// The elements of both sorted lists, sorted.
fn intersect(a: &[ElementId], b: &[ElementId]) -> Vec<ElementId> {
    let (mut i, mut j, mut out) = (a.iter().peekable(), b.iter().peekable(), Vec::new());
    while let (Some(&&x), Some(&&y)) = (i.peek(), j.peek()) {
        match x.cmp(&y) {
            std::cmp::Ordering::Less => {
                i.next();
            }
            std::cmp::Ordering::Greater => {
                j.next();
            }
            std::cmp::Ordering::Equal => {
                out.push(x);
                i.next();
                j.next();
            }
        }
    }
    out
}
