//! \[EXTENSION\] LOUVAIN (the graph standard's §18, `GRAPH_ALGO.LOUVAIN(g)`):
//! communities of the graph's undirected form by the method of Blondel,
//! Guillaume, Lambiotte and Lefebvre ("Fast unfolding of communities in
//! large networks", J. Stat. Mech., 2008). Each level moves each vertex
//! in turn to the neighbouring community of the greatest gain of
//! modularity, of the resolution γ of Reichardt and Bornholdt, until a
//! pass moves none or gains less than the tolerance; then each community
//! becomes a vertex of the next level, its edges summed, its own edges a
//! loop. It stops when a level moves no vertex or after `levels` levels.
//!
//! The order a level visits its vertices in is the seed's (`random.rs`),
//! so a seed gives the same communities on every run. A level's moves
//! are made one after another, each seeing the last; the next level's
//! graph is built in morsels of communities, each summing its members'
//! edges in their order, and the modularity is summed by morsel and the
//! morsels' sums in order, so every width gives the same communities and
//! the same modularity to the bit. Each community is labelled by its
//! least vertex.

use super::control::{Cancel, Control};
use super::csr::{Undirected, Vertex, ix, vertex};
use super::invalid;
use super::parallel::{MORSEL, morsels};
use super::random::Random;
use crate::error::Result;

/// The resolution of plain modularity.
pub const RESOLUTION: f64 = 1.0_f64;
/// \[DESIGN\] The most levels: few graphs need more than a handful.
pub const LEVELS: u32 = 10;
/// \[DESIGN\] The least gain of modularity a pass of moves must make for
/// another to follow.
pub const TOLERANCE: f64 = 1e-7_f64;
/// \[DESIGN\] The most passes of moves a level makes.
const PASSES: u32 = 100;
/// A loop's weight counts at both of its ends in a vertex's strength.
const LOOP_ENDS: f64 = 2.0_f64;
/// A community not yet numbered.
const UNSET: Vertex = Vertex::MAX;
/// The arrays of a vertex a level holds: its strength, community,
/// community's total, scratch weight, loops and offset.
const ARRAYS: usize = 6;

/// LOUVAIN's parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Louvain {
    /// The resolution γ, more than 0: higher gives smaller communities.
    pub resolution: f64,
    /// The most levels.
    pub levels: u32,
    /// The least gain of modularity of a pass for another to follow.
    pub tolerance: f64,
    /// The seed of the order vertices are visited in.
    pub seed: u64,
}

impl Default for Louvain {
    fn default() -> Self {
        Louvain { resolution: RESOLUTION, levels: LEVELS, tolerance: TOLERANCE, seed: 0 }
    }
}

/// LOUVAIN's result: each vertex's community, by its least vertex; their
/// modularity; and the levels that moved vertices.
#[derive(Debug, Clone, PartialEq)]
pub struct Communities {
    /// The community of each vertex.
    pub labels: Vec<Vertex>,
    /// The communities' modularity, of the resolution.
    pub modularity: f64,
    /// The levels that moved vertices.
    pub levels: u32,
}

/// One level's graph: of each vertex its neighbours and their weights,
/// and its loops.
#[derive(Debug, Clone, Default)]
struct Level {
    offsets: Vec<usize>,
    neighbours: Vec<Vertex>,
    weights: Vec<f64>,
    loops: Vec<f64>,
}

impl Level {
    /// The first level, `u` itself.
    fn of(u: &Undirected<'_>) -> Level {
        let mut level = Level { offsets: vec![0], ..Level::default() };
        for v in (0..u.vertices()).map(vertex) {
            level.neighbours.extend_from_slice(u.neighbours(v));
            level.weights.extend_from_slice(u.weights(v));
            level.loops.push(u.loops(v));
            level.offsets.push(level.neighbours.len());
        }
        level
    }

    /// Its vertices.
    fn vertices(&self) -> usize {
        self.loops.len()
    }

    /// The neighbours of `i` and their weights.
    fn edges(&self, i: usize) -> impl Iterator<Item = (Vertex, f64)> + '_ {
        let lo = self.offsets.get(i).copied().unwrap_or(0);
        let hi = self.offsets.get(i.saturating_add(1)).copied().unwrap_or(lo);
        let ns = self.neighbours.get(lo..hi).unwrap_or_default();
        ns.iter().copied().zip(self.weights.get(lo..hi).unwrap_or_default().iter().copied())
    }

    /// The strength of `i`: its edges' weights, its loops' twice.
    fn strength(&self, i: usize) -> f64 {
        self.edges(i).map(|(_, w)| w).sum::<f64>() + LOOP_ENDS * self.loops.get(i).copied().unwrap_or(0.0_f64)
    }
}

/// The communities of `u`.
///
/// # Errors
///
/// SQLCODE -301 if the resolution is not more than 0 or the tolerance is
/// negative; -918 past the budget; the cancel's.
pub fn louvain<C: Cancel>(u: &Undirected<'_>, p: &Louvain, control: &Control<'_, C>) -> Result<Communities> {
    if !(p.resolution > 0.0_f64 && p.resolution.is_finite()) {
        return Err(invalid("LOUVAIN", "resolution", "more than 0"));
    }
    if p.tolerance.is_nan() || p.tolerance < 0.0_f64 {
        return Err(invalid("LOUVAIN", "tolerance", "0 or more"));
    }
    let n = u.vertices();
    let bytes =
        u.entries().saturating_mul(size_of::<(Vertex, f64)>()).saturating_add(n.saturating_mul(ARRAYS).saturating_mul(size_of::<f64>()));
    let _charge = control.memory.charge(bytes, "LOUVAIN's levels")?;
    let mut level = Level::of(u);
    let mut mapping: Vec<Vertex> = (0..n).map(vertex).collect();
    let mut random = Random::new(p.seed);
    let mut levels = 0;
    while levels < p.levels {
        control.check()?;
        let Some(communities) = moving(&level, p, &mut random, control)? else { break };
        levels = levels.saturating_add(1);
        let (dense, count) = renumbered(&communities);
        for m in &mut mapping {
            *m = dense.get(ix(*m)).copied().unwrap_or(UNSET);
        }
        level = aggregate(&level, (&dense, count), control)?;
    }
    let labels = least(&mapping, n);
    let modularity = modularity(u, &labels, p.resolution, control)?;
    Ok(Communities { labels, modularity, levels })
}

/// A level's moves, pass after pass: each vertex's community, if any
/// vertex moved.
fn moving<C: Cancel>(level: &Level, p: &Louvain, random: &mut Random, control: &Control<'_, C>) -> Result<Option<Vec<Vertex>>> {
    let n = level.vertices();
    let strengths: Vec<f64> = (0..n).map(|i| level.strength(i)).collect();
    let total: f64 = strengths.iter().sum();
    if total <= 0.0_f64 {
        return Ok(None);
    }
    let order = random.shuffled(n);
    let mut m = Moves {
        community: (0..n).map(vertex).collect(),
        totals: strengths.clone(),
        weight_to: vec![0.0_f64; n],
        seen: vec![false; n],
        touched: Vec::new(),
    };
    let mut moved = false;
    let mut score = quality(level, &m.community, &m.totals, (total, p.resolution));
    for _ in 0..PASSES {
        control.check()?;
        let mut shifted = 0_usize;
        for &i in &order {
            let strength = strengths.get(ix(i)).copied().unwrap_or(0.0_f64);
            if m.best(level, i, (strength, total, p.resolution)) {
                shifted = shifted.saturating_add(1);
            }
        }
        if shifted == 0 {
            break;
        }
        moved = true;
        let next = quality(level, &m.community, &m.totals, (total, p.resolution));
        let gained = next - score;
        score = next;
        if gained < p.tolerance {
            break;
        }
    }
    Ok(moved.then_some(m.community))
}

/// The state of a level's moves.
struct Moves {
    community: Vec<Vertex>,
    totals: Vec<f64>,
    weight_to: Vec<f64>,
    seen: Vec<bool>,
    touched: Vec<Vertex>,
}

impl Moves {
    /// Moves `i`, of `strength`, of a level of `total` strength, to the
    /// community of the greatest gain at `resolution`, its own unless
    /// another's is greater, the first found between equals: whether it
    /// moved.
    fn best(&mut self, level: &Level, i: Vertex, (strength, total, resolution): (f64, f64, f64)) -> bool {
        for (j, w) in level.edges(ix(i)) {
            let c = self.community.get(ix(j)).copied().unwrap_or(UNSET);
            if let Some(seen) = self.seen.get_mut(ix(c)).filter(|s| !**s) {
                *seen = true;
                self.touched.push(c);
            }
            if let Some(x) = self.weight_to.get_mut(ix(c)) {
                *x += w;
            }
        }
        let own = self.community.get(ix(i)).copied().unwrap_or(UNSET);
        add(&mut self.totals, own, -strength);
        let gain = |c: Vertex, m: &Moves| {
            m.weight_to.get(ix(c)).copied().unwrap_or(0.0_f64)
                - resolution * m.totals.get(ix(c)).copied().unwrap_or(0.0_f64) * strength / total
        };
        let (mut chosen, mut most) = (own, gain(own, self));
        for &c in &self.touched {
            let g = gain(c, self);
            if g > most {
                (chosen, most) = (c, g);
            }
        }
        add(&mut self.totals, chosen, strength);
        if let Some(c) = self.community.get_mut(ix(i)) {
            *c = chosen;
        }
        for c in self.touched.drain(..) {
            if let Some(x) = self.weight_to.get_mut(ix(c)) {
                *x = 0.0_f64;
            }
            if let Some(s) = self.seen.get_mut(ix(c)) {
                *s = false;
            }
        }
        chosen != own
    }
}

/// Adds `x` at `c` of `of`.
fn add(of: &mut [f64], c: Vertex, x: f64) {
    if let Some(t) = of.get_mut(ix(c)) {
        *t += x;
    }
}

/// The modularity of a level's `community`, of community `totals`, of a
/// level of `total` strength at `resolution`.
fn quality(level: &Level, community: &[Vertex], totals: &[f64], (total, resolution): (f64, f64)) -> f64 {
    let mut inner = 0.0_f64;
    for (i, &c) in community.iter().enumerate() {
        inner += level.edges(i).filter(|(j, _)| community.get(ix(*j)) == Some(&c)).map(|(_, w)| w).sum::<f64>();
        inner += LOOP_ENDS * level.loops.get(i).copied().unwrap_or(0.0_f64);
    }
    let spread: f64 = totals.iter().map(|t| (t / total) * (t / total)).sum();
    inner / total - resolution * spread
}

/// Each vertex's community numbered densely, in the order of their least
/// vertices; and how many.
fn renumbered(community: &[Vertex]) -> (Vec<Vertex>, usize) {
    let mut numbers = vec![UNSET; community.len()];
    let mut next = 0_u32;
    let mut dense = Vec::with_capacity(community.len());
    for &c in community {
        let Some(number) = numbers.get_mut(ix(c)) else { continue };
        if *number == UNSET {
            *number = next;
            next = next.saturating_add(1);
        }
        dense.push(*number);
    }
    (dense, ix(next))
}

/// The next level of `level`, of its vertices' dense communities and
/// their `count`.
fn aggregate<C: Cancel>(level: &Level, (dense, count): (&[Vertex], usize), control: &Control<'_, C>) -> Result<Level> {
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (i, &c) in dense.iter().enumerate() {
        if let Some(m) = members.get_mut(ix(c)) {
            m.push(i);
        }
    }
    let parts = morsels(control, (count, MORSEL), |r| {
        let mut part = Vec::with_capacity(r.len());
        for c in r {
            part.push(community(level, dense, (c, members.get(c).map(Vec::as_slice).unwrap_or_default())));
        }
        Ok(part)
    })?;
    let mut next = Level { offsets: vec![0], ..Level::default() };
    for (loops, edges) in parts.into_iter().flatten() {
        next.loops.push(loops);
        for (d, w) in edges {
            next.neighbours.push(d);
            next.weights.push(w);
        }
        next.offsets.push(next.neighbours.len());
    }
    Ok(next)
}

/// Of community `c` of `members`, its vertex of the next level: its
/// loops, its own members' loops and the edges among them; and its
/// edges to the other communities, summed, by community.
fn community(level: &Level, dense: &[Vertex], (c, members): (usize, &[usize])) -> (f64, Vec<(Vertex, f64)>) {
    let (mut loops, mut inner) = (0.0_f64, 0.0_f64);
    let mut out: Vec<(Vertex, f64)> = Vec::new();
    for &i in members {
        loops += level.loops.get(i).copied().unwrap_or(0.0_f64);
        for (j, w) in level.edges(i) {
            let d = dense.get(ix(j)).copied().unwrap_or(UNSET);
            if ix(d) == c {
                inner += w;
            } else {
                out.push((d, w));
            }
        }
    }
    out.sort_by_key(|(d, _)| *d);
    (loops + inner / LOOP_ENDS, summed(out))
}

/// `sorted`'s weights summed for each vertex, in order.
fn summed(sorted: Vec<(Vertex, f64)>) -> Vec<(Vertex, f64)> {
    let mut out: Vec<(Vertex, f64)> = Vec::with_capacity(sorted.len());
    for (d, w) in sorted {
        if let Some(last) = out.last_mut().filter(|l| l.0 == d) {
            last.1 += w;
        } else {
            out.push((d, w));
        }
    }
    out
}

/// Each vertex's community by its least vertex, of the communities
/// `mapping` numbers.
fn least(mapping: &[Vertex], n: usize) -> Vec<Vertex> {
    let mut first = vec![UNSET; n];
    mapping
        .iter()
        .enumerate()
        .map(|(v, &c)| match first.get_mut(ix(c)) {
            Some(f) => {
                if *f == UNSET {
                    *f = vertex(v);
                }
                *f
            }
            None => vertex(v),
        })
        .collect()
}

/// The modularity of `labels` on `u` at `resolution`, summed by morsel.
fn modularity<C: Cancel>(u: &Undirected<'_>, labels: &[Vertex], resolution: f64, control: &Control<'_, C>) -> Result<f64> {
    let n = u.vertices();
    let parts = morsels(control, (n, MORSEL), |r| {
        let (mut inner, mut strength) = (0.0_f64, Vec::with_capacity(r.len()));
        for v in r.map(vertex) {
            let mine = labels.get(ix(v));
            let loops = LOOP_ENDS * u.loops(v);
            let same = u.neighbours(v).iter().zip(u.weights(v)).filter(|(j, _)| labels.get(ix(**j)) == mine);
            inner += same.map(|(_, w)| w).sum::<f64>() + loops;
            strength.push(u.weights(v).iter().sum::<f64>() + loops);
        }
        Ok((inner, strength))
    })?;
    let mut totals = vec![0.0_f64; n];
    let (mut inner, mut total) = (0.0_f64, 0.0_f64);
    for (k, (i, strengths)) in parts.into_iter().enumerate() {
        inner += i;
        for (offset, s) in strengths.into_iter().enumerate() {
            total += s;
            let v = k.saturating_mul(MORSEL).saturating_add(offset);
            add(&mut totals, labels.get(v).copied().unwrap_or(UNSET), s);
        }
    }
    if total <= 0.0_f64 {
        return Ok(0.0_f64);
    }
    Ok(inner / total - resolution * totals.iter().map(|t| (t / total) * (t / total)).sum::<f64>())
}
