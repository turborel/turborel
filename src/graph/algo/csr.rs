//! \[DESIGN\] The graph an algorithm runs on: compressed sparse rows of
//! dense vertex numbers, built from the edges, each a source, a
//! destination and perhaps a weight. Each vertex has a span of its
//! outgoing edges and one of its incoming edges, each sorted by the other
//! end, so every run reads them in the same order whatever order the
//! edges came in. A parallel edge or a loop is kept as given; the
//! undirected form an algorithm of undirected graphs reads
//! (`Undirected`) has neither, a pair's edges one neighbour of summed
//! weight.

use std::cmp::Ordering;

use super::control::{Cancel, Charge, Control, Memory};
use super::parallel::{MORSEL, morsels};
use crate::error::{Code, Result, SqlError};

/// A vertex, by its dense number.
pub type Vertex = u32;

/// The bytes an edge takes as the builder holds it.
const EDGE_BYTES: usize = size_of::<(Vertex, Vertex, f64)>();

/// The directions an edge is kept in: each edge is in one span of each.
const DIRECTIONS: usize = 2;

/// The bytes a neighbour takes in the undirected form, a number and a
/// weight; a vertex there takes as many, an offset and its loops.
const NEIGHBOUR_BYTES: usize = size_of::<Vertex>() + size_of::<f64>();

/// The bytes of a vertex's offsets, one in each direction.
const OFFSET_BYTES: usize = DIRECTIONS * size_of::<u32>();

/// A loop is in both of its vertex's spans, so is met twice.
const LOOP_SIDES: f64 = 2.0_f64;

/// `v` as a place in an array: a number past any array if it fits none.
pub(crate) fn ix(v: Vertex) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

/// The vertex of place `i`, the greatest number if past them.
pub(crate) fn vertex(i: usize) -> Vertex {
    Vertex::try_from(i).unwrap_or(Vertex::MAX)
}

/// A count as a float: exact, as a graph's counts are 32-bit.
pub(crate) fn real(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// The error of a graph past 32-bit numbers.
fn too_big(what: &str) -> SqlError {
    SqlError::new(Code::Resources, format!("[DESIGN] a graph algorithm's graph has more {what} than 4294967294"))
}

/// One direction's spans: of each vertex its run of `ends`, the other
/// end of each edge, and their weights if weighted.
#[derive(Debug, Default)]
struct Side {
    offsets: Vec<u32>,
    ends: Vec<Vertex>,
    weights: Option<Vec<f64>>,
}

impl Side {
    /// The span of `v`.
    fn span(&self, v: Vertex) -> (usize, usize) {
        let i = ix(v);
        let at = |k: usize| self.offsets.get(k).map_or(0, |&o| ix(o));
        (at(i), at(i.saturating_add(1)))
    }

    /// The other ends of `v`'s edges.
    fn ends(&self, v: Vertex) -> &[Vertex] {
        let (lo, hi) = self.span(v);
        self.ends.get(lo..hi).unwrap_or_default()
    }

    /// The weights of `v`'s edges, if weighted.
    fn weights(&self, v: Vertex) -> Option<&[f64]> {
        let (lo, hi) = self.span(v);
        self.weights.as_ref().map(|w| w.get(lo..hi).unwrap_or_default())
    }

    /// The spans of `n` vertices of `edges`, sorted by `key`: the first of
    /// the pair whose span it is, then the other end.
    fn of(n: usize, edges: &[(Vertex, Vertex, f64)], weighted: bool, key: fn(&(Vertex, Vertex, f64)) -> (Vertex, Vertex)) -> Result<Side> {
        let mut offsets = vec![0_u32; n.saturating_add(1)];
        for e in edges {
            if let Some(o) = offsets.get_mut(ix(key(e).0).saturating_add(1)) {
                *o = o.saturating_add(1);
            }
        }
        let mut total = 0_u32;
        for o in &mut offsets {
            total = total.checked_add(*o).ok_or_else(|| too_big("edges"))?;
            *o = total;
        }
        let ends = edges.iter().map(|e| key(e).1).collect();
        let weights = weighted.then(|| edges.iter().map(|e| e.2).collect());
        Ok(Side { offsets, ends, weights })
    }
}

/// A directed graph of dense vertex numbers in compressed sparse rows.
#[derive(Debug)]
pub struct Graph<'m> {
    vertices: usize,
    out: Side,
    ins: Side,
    _charge: Charge<'m>,
}

impl Graph<'_> {
    /// Its vertices, numbered from 0.
    pub fn vertices(&self) -> usize {
        self.vertices
    }

    /// Its edges.
    pub fn edges(&self) -> usize {
        self.out.ends.len()
    }

    /// Whether its edges have weights.
    pub fn weighted(&self) -> bool {
        self.out.weights.is_some()
    }

    /// The destinations of `v`'s outgoing edges, in order.
    pub fn out(&self, v: Vertex) -> &[Vertex] {
        self.out.ends(v)
    }

    /// The weights of `v`'s outgoing edges, in the order of `out`.
    pub fn out_weights(&self, v: Vertex) -> Option<&[f64]> {
        self.out.weights(v)
    }

    /// The sources of `v`'s incoming edges, in order.
    pub fn ins(&self, v: Vertex) -> &[Vertex] {
        self.ins.ends(v)
    }

    /// The weights of `v`'s incoming edges, in the order of `ins`.
    pub fn in_weights(&self, v: Vertex) -> Option<&[f64]> {
        self.ins.weights(v)
    }

    /// The number of `v`'s outgoing edges.
    pub fn out_degree(&self, v: Vertex) -> usize {
        self.out(v).len()
    }

    /// Its undirected form, built of `control`'s budget.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 past the budget; the cancel's.
    pub fn undirected<'c, C: Cancel>(&self, control: &Control<'c, C>) -> Result<Undirected<'c>> {
        undirected(self, control)
    }
}

/// Builds a `Graph` of edges given one at a time.
#[derive(Debug)]
pub struct GraphBuilder<'m> {
    memory: &'m Memory,
    vertices: usize,
    weighted: bool,
    edges: Vec<(Vertex, Vertex, f64)>,
    charges: Vec<Charge<'m>>,
}

impl<'m> GraphBuilder<'m> {
    /// A builder of a graph of `vertices` vertices, numbered from 0, whose
    /// edges have weights if `weighted`, of `memory`.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if `vertices` is past 32-bit numbers.
    pub fn new(memory: &'m Memory, vertices: usize, weighted: bool) -> Result<GraphBuilder<'m>> {
        if vertices >= ix(Vertex::MAX) {
            return Err(too_big("vertices"));
        }
        Ok(GraphBuilder { memory, vertices, weighted, edges: Vec::new(), charges: Vec::new() })
    }

    /// Adds the edge from `source` to `destination` of `weight`, which an
    /// unweighted graph ignores.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 past the budget; -910 if a vertex is not of the graph.
    pub fn edge(&mut self, source: Vertex, destination: Vertex, weight: f64) -> Result<()> {
        if ix(source) >= self.vertices || ix(destination) >= self.vertices {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] an edge from {source} to {destination} is of no vertex of the graph")));
        }
        if self.edges.len() == self.edges.capacity() {
            let grow = self.edges.capacity().max(MORSEL);
            self.charges.push(self.memory.charge(grow.saturating_mul(EDGE_BYTES), "its edges")?);
            self.edges.reserve_exact(grow);
        }
        self.edges.push((source, destination, if self.weighted { weight } else { 1.0_f64 }));
        Ok(())
    }

    /// The graph.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 past the budget or past 32-bit numbers of edges.
    pub fn build(self) -> Result<Graph<'m>> {
        let GraphBuilder { memory, vertices, weighted, mut edges, charges } = self;
        if edges.len() >= ix(Vertex::MAX) {
            return Err(too_big("edges"));
        }
        let per_edge = size_of::<Vertex>().saturating_add(if weighted { size_of::<f64>() } else { 0 }).saturating_mul(DIRECTIONS);
        let bytes = edges.len().saturating_mul(per_edge).saturating_add(vertices.saturating_mul(OFFSET_BYTES));
        let charge = memory.charge(bytes, "its compressed sparse rows")?;
        let weight = |a: &(Vertex, Vertex, f64), b: &(Vertex, Vertex, f64)| a.2.total_cmp(&b.2);
        edges.sort_unstable_by(|a, b| (a.1, a.0).cmp(&(b.1, b.0)).then_with(|| weight(a, b)));
        let ins = Side::of(vertices, &edges, weighted, |e| (e.1, e.0))?;
        edges.sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then_with(|| weight(a, b)));
        let out = Side::of(vertices, &edges, weighted, |e| (e.0, e.1))?;
        drop(edges);
        drop(charges);
        Ok(Graph { vertices, out, ins, _charge: charge })
    }
}

/// The graph of `vertices` vertices and `edges`, weighted if `weighted`,
/// of `memory`.
///
/// # Errors
///
/// As `GraphBuilder`'s.
pub fn graph(
    memory: &Memory,
    (vertices, weighted): (usize, bool),
    edges: impl IntoIterator<Item = (Vertex, Vertex, f64)>,
) -> Result<Graph<'_>> {
    let mut b = GraphBuilder::new(memory, vertices, weighted)?;
    for (s, d, w) in edges {
        b.edge(s, d, w)?;
    }
    b.build()
}

/// A graph's undirected form: of each vertex its neighbours, sorted, no
/// vertex twice nor itself, each of the summed weights of the edges
/// between them in either direction (of an unweighted graph, their
/// count).
#[derive(Debug)]
pub struct Undirected<'m> {
    offsets: Vec<u32>,
    neighbours: Vec<Vertex>,
    weights: Vec<f64>,
    loops: Vec<f64>,
    _charge: Charge<'m>,
}

impl Undirected<'_> {
    /// Its vertices.
    pub fn vertices(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// The span of `v`.
    fn span(&self, v: Vertex) -> (usize, usize) {
        let i = ix(v);
        let at = |k: usize| self.offsets.get(k).map_or(0, |&o| ix(o));
        (at(i), at(i.saturating_add(1)))
    }

    /// The neighbours of `v`, sorted.
    pub fn neighbours(&self, v: Vertex) -> &[Vertex] {
        let (lo, hi) = self.span(v);
        self.neighbours.get(lo..hi).unwrap_or_default()
    }

    /// The weights of `v`'s neighbours, in the order of `neighbours`.
    pub fn weights(&self, v: Vertex) -> &[f64] {
        let (lo, hi) = self.span(v);
        self.weights.get(lo..hi).unwrap_or_default()
    }

    /// The summed weight of `v`'s loops, of an edge from it to itself.
    pub fn loops(&self, v: Vertex) -> f64 {
        self.loops.get(ix(v)).copied().unwrap_or(0.0_f64)
    }

    /// Its entries: each edge's two ends, in each other's neighbours.
    pub fn entries(&self) -> usize {
        self.neighbours.len()
    }

    /// The number of `v`'s neighbours.
    pub fn degree(&self, v: Vertex) -> usize {
        self.neighbours(v).len()
    }
}

/// One morsel's part of an undirected form: its neighbours, their
/// weights, each vertex's degree and loops.
type Part = (Vec<Vertex>, Vec<f64>, Vec<u32>, Vec<f64>);

/// The undirected form of `g`.
fn undirected<'c, C: Cancel>(g: &Graph<'_>, control: &Control<'c, C>) -> Result<Undirected<'c>> {
    let n = g.vertices();
    let bytes = g.edges().saturating_mul(DIRECTIONS * NEIGHBOUR_BYTES).saturating_add(n.saturating_mul(NEIGHBOUR_BYTES));
    let charge = control.memory.charge(bytes, "the undirected form")?;
    let parts = morsels(control, (n, MORSEL), |r| {
        let mut part: Part = (Vec::new(), Vec::new(), Vec::with_capacity(r.len()), Vec::with_capacity(r.len()));
        for v in r.map(vertex) {
            let before = part.0.len();
            let loops = merged(g, v, &mut part.0, &mut part.1);
            part.2.push(vertex(part.0.len().saturating_sub(before)));
            part.3.push(loops);
        }
        Ok(part)
    })?;
    let (mut offsets, mut neighbours, mut weights, mut loops) =
        (Vec::with_capacity(n.saturating_add(1)), Vec::new(), Vec::new(), Vec::new());
    offsets.push(0_u32);
    let mut total = 0_u32;
    for (ns, ws, degrees, ls) in parts {
        for d in degrees {
            total = total.checked_add(d).ok_or_else(|| too_big("edges"))?;
            offsets.push(total);
        }
        neighbours.extend(ns);
        weights.extend(ws);
        loops.extend(ls);
    }
    Ok(Undirected { offsets, neighbours, weights, loops, _charge: charge })
}

/// Pushes the neighbours of `v` in `g` and their weights, merging its
/// outgoing and incoming spans, each sorted; its loops' weight, which
/// each loop gives once though it is in both spans.
fn merged(g: &Graph<'_>, v: Vertex, neighbours: &mut Vec<Vertex>, weights: &mut Vec<f64>) -> f64 {
    let ones_out = vec![1.0_f64; g.out(v).len()];
    let ones_in = vec![1.0_f64; g.ins(v).len()];
    let mut outs = g.out(v).iter().zip(g.out_weights(v).unwrap_or(&ones_out)).peekable();
    let mut ins = g.ins(v).iter().zip(g.in_weights(v).unwrap_or(&ones_in)).peekable();
    let (start, mut loops) = (neighbours.len(), 0.0_f64);
    loop {
        let next = match (outs.peek(), ins.peek()) {
            (Some(o), Some(i)) => match o.0.cmp(i.0) {
                Ordering::Greater => ins.next(),
                Ordering::Less | Ordering::Equal => outs.next(),
            },
            (Some(_), None) => outs.next(),
            (None, _) => ins.next(),
        };
        let Some((&end, &weight)) = next else { return loops / LOOP_SIDES };
        if end == v {
            loops += weight;
        } else if neighbours.len() > start && neighbours.last() == Some(&end) {
            if let Some(last) = weights.last_mut() {
                *last += weight;
            }
        } else {
            neighbours.push(end);
            weights.push(weight);
        }
    }
}
