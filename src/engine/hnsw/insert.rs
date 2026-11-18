//! \[DESIGN\] HNSW insertion (Malkov and Yashunin, §4, Algorithm 1) and
//! its neighbour selection by heuristic (Algorithm 4): a candidate is kept
//! only if it is nearer the base than every neighbour kept before it, so
//! the links reach out in different directions; extendCandidates adds the
//! candidates' own neighbours first, keepPrunedConnections fills the list
//! up to its most from those set aside.
//!
//! An insertion is planned, then published (R-ERR-4, `view`). Every
//! layer's search and selection read the graph through a `Source`, with
//! no lock held as the metric runs; then, under the lock, the node and
//! its lists are added in one change, and its back edges where they fit.
//! A neighbour whose list is full has it chosen again the same way: read,
//! selected with no lock held, and written only if it is still as read.

use std::mem::take;

use super::graph::{Graph, NONE};
use super::measure::Metric;
use super::scratch::{Planned, Scored, Scratch};
use super::search::{descend, search_layer};
use super::view::{Source, Step};

/// How neighbours are chosen as nodes are linked.
#[derive(Debug, Clone, Copy)]
pub struct Build {
    /// efConstruction: the candidates an insertion searches for.
    pub ef_construction: usize,
    /// extendCandidates.
    pub extend: bool,
    /// keepPrunedConnections.
    pub keep_pruned: bool,
    /// M: the most neighbours a node keeps on an upper layer.
    pub m: usize,
    /// M0: the most it keeps on layer 0.
    pub m0: usize,
}

impl Build {
    /// The most neighbours a node keeps on `level`: M0 on layer 0, else M.
    pub fn most(&self, level: u8) -> usize {
        if level == 0 { self.m0 } else { self.m }
    }
}

/// The `at`th vector of `vecs`, `dims` each.
#[inline]
fn vector_at(vecs: &[f32], dims: usize, at: usize) -> &[f32] {
    vecs.get(at.saturating_mul(dims)..at.saturating_add(1).saturating_mul(dims)).unwrap_or_default()
}

/// SELECT-NEIGHBORS-HEURISTIC (Algorithm 4): of the candidates in
/// `s.chosen`, sorted nearest `base` first, those kept, in `s.chosen`,
/// and how many the heuristic kept, in `s.known`. If `fetched`, `s.vecs`
/// holds their vectors already, found by `s.place`.
///
/// # Errors
///
/// The source's.
pub fn select<S: Source>(
    src: &mut S,
    metric: &Metric,
    base: &[f32],
    (level, build, fetched): (u8, &Build, bool),
    s: &mut Scratch,
) -> Step<()> {
    if build.extend {
        extend(src, metric, base, level, s)?;
    }
    if build.extend || !fetched {
        s.ids.clear();
        s.ids.extend(s.chosen.iter().map(|c| c.id));
        src.vectors(&s.ids, &mut s.vecs)?;
        s.place.clear();
        s.place.extend(0..s.chosen.len());
    }
    heuristic(metric, (src.dims(), build.most(level), build.keep_pruned), s);
    Ok(())
}

/// The heuristic's choice itself, over vectors fetched. Each pair is
/// measured at most once: a candidate against each neighbour kept, until
/// one is nearer it than the base is.
fn heuristic(metric: &Metric, (dims, most, keep_pruned): (usize, usize, bool), s: &mut Scratch) {
    let Scratch { chosen, kept, pruned, vecs, place, kept_at, known, .. } = s;
    kept.clear();
    pruned.clear();
    kept_at.clear();
    for (e, at) in chosen.iter().zip(place.iter()) {
        if kept.len() >= most {
            break;
        }
        let ve = vector_at(vecs, dims, *at);
        if kept_at.iter().all(|r| metric.distance(ve, vector_at(vecs, dims, *r)) > e.distance) {
            kept.push(*e);
            kept_at.push(*at);
        } else {
            pruned.push(*e);
        }
    }
    *known = Some(kept.len());
    if keep_pruned {
        let room = most.saturating_sub(kept.len());
        kept.extend(pruned.iter().take(room));
    }
    std::mem::swap(chosen, kept);
}

/// extendCandidates: each candidate's neighbours on `level` join them.
fn extend<S: Source>(src: &mut S, metric: &Metric, base: &[f32], level: u8, s: &mut Scratch) -> Step<()> {
    let Scratch { visited, chosen, .. } = s;
    visited.begin(0);
    for c in chosen.iter() {
        visited.first(c.id);
    }
    let count = chosen.len();
    for at in 0..count {
        let Some(c) = chosen.get(at).copied() else { break };
        src.hop((c.id, level), visited, &mut |e, _, v| chosen.push(Scored { distance: metric.distance(base, v), id: e }))?;
    }
    chosen.sort_unstable();
    Ok(())
}

/// INSERT (Algorithm 1), planned: the lists of a node of `q` up to layer
/// `level`, in `s.plan`, top layer first, chosen by searching from the
/// entry; the entry searched from, `NONE` if the graph was empty.
///
/// # Errors
///
/// The source's.
pub fn plan<S: Source>(src: &mut S, metric: &Metric, (q, level): (&[f32], u8), build: &Build, s: &mut Scratch) -> Step<u32> {
    let mut plan = take(&mut s.plan);
    plan.clear();
    let mut entries = take(&mut s.entries);
    let planned = src.entry().and_then(|(entry, top)| {
        layers(src, (metric, q, level), (entry, top, build), s, (&mut entries, &mut plan))?;
        Ok(entry)
    });
    s.plan = plan;
    s.entries = entries;
    planned
}

/// The layers of `plan`, from the lower of `level` and `top` down, each
/// searched from `entries`, the layer above's nearest.
fn layers<S: Source>(
    src: &mut S,
    (metric, q, level): (&Metric, &[f32], u8),
    (entry, top, build): (u32, u8, &Build),
    s: &mut Scratch,
    (entries, plan): (&mut Vec<Scored>, &mut Vec<Planned>),
) -> Step<()> {
    let Some(start) = descend(src, (metric, q), (entry, top, level), s)? else { return Ok(()) };
    entries.clear();
    entries.push(start);
    let mut at = level.min(top);
    loop {
        s.visited.begin(0);
        search_layer(src, (metric, q), (entries, build.ef_construction, at), s, &mut |_, _| true)?;
        s.chosen.clear();
        s.chosen.extend(s.results.drain());
        s.chosen.sort_unstable();
        entries.clear();
        entries.extend_from_slice(&s.chosen);
        select(src, metric, q, (at, build, false), s)?;
        plan.push((at, s.chosen.iter().map(|c| c.id).collect(), s.known));
        let Some(below) = at.checked_sub(1) else { break };
        at = below;
    }
    Ok(())
}

/// What a node inserted stores: its floats, or its code.
pub enum Payload<'a> {
    /// Full precision.
    Floats(&'a [f32]),
    /// A code of the graph's codec.
    Code(&'a [u8]),
}

impl Graph {
    /// Publishes an insertion planned in `s.plan`: the node of `row` and
    /// `hash`, up to layer `level`, storing `payload`, with its lists and
    /// its back edges where they fit; those that do not are left in
    /// `s.over`. Makes it the entry if it is the first or tops the
    /// others. Calls no plug-in, so it is made under the lock.
    pub fn publish(&mut self, (row, hash, level): (u64, u64, u8), payload: &Payload<'_>, s: &mut Scratch) -> u32 {
        let id = match payload {
            Payload::Floats(v) => self.push((row, hash, 1), level, v),
            Payload::Code(code) => self.push_code((row, hash, 1), level, code),
        };
        s.over.clear();
        for (at, ids, kept) in &s.plan {
            self.set_links_kept(id, *at, ids, *kept);
        }
        for (at, ids, _) in &s.plan {
            for &e in ids {
                if !self.push_link(e, *at, id) {
                    s.over.push((e, *at));
                }
            }
        }
        if self.entry == NONE || level > self.top {
            self.entry = id;
            self.top = level;
        }
        self.changes = self.changes.wrapping_add(1);
        id
    }

    /// Makes node `e`'s list on `level` `s.chosen`'s ids, if it is still
    /// `s.before`: whether it was.
    pub fn apply(&mut self, (e, level): (u32, u8), s: &mut Scratch) -> bool {
        if self.links(e, level) != s.before.as_slice() {
            return false;
        }
        s.spare.clear();
        s.spare.extend(s.chosen.iter().map(|c| c.id));
        self.set_links_kept(e, level, &s.spare, s.known);
        self.changes = self.changes.wrapping_add(1);
        true
    }
}

/// The candidates `s.ids`, their vectors in `s.vecs`, measured from
/// `base` and sorted nearest first into `s.chosen`, each one's vector
/// found by `s.place`.
pub fn measured(metric: &Metric, base: &[f32], dims: usize, s: &mut Scratch) {
    let Scratch { ids, vecs, chosen, place, pairs, .. } = s;
    pairs.clear();
    pairs.extend(ids.iter().enumerate().map(|(at, n)| (Scored { distance: metric.distance(base, vector_at(vecs, dims, at)), id: *n }, at)));
    // The ids are distinct, so the order is total.
    pairs.sort_unstable_by_key(|p| p.0);
    chosen.clear();
    place.clear();
    for (c, at) in pairs.iter() {
        chosen.push(*c);
        place.push(*at);
    }
}

/// Chooses node `e`'s list on `level` again by the heuristic, from its
/// neighbours and `new`, into `s.chosen`; the list read, in `s.before`.
///
/// # Errors
///
/// The source's.
pub fn reselect<S: Source>(src: &mut S, metric: &Metric, (e, level, new): (u32, u8, u32), build: &Build, s: &mut Scratch) -> Step<()> {
    let mut before = take(&mut s.before);
    let mut base = take(&mut s.base);
    let chosen =
        src.links((e, level), &mut before).and_then(|kept| choose(src, metric, (&mut base, &before, kept), ((e, level, new), build), s));
    s.base = base;
    s.before = before;
    chosen
}

/// `reselect` from the list read, `before`, measured from `base`, its
/// first `kept` those the heuristic kept, if known.
fn choose<S: Source>(
    src: &mut S,
    metric: &Metric,
    (base, before, kept): (&mut Vec<f32>, &[u32], Option<usize>),
    ((e, level, new), build): ((u32, u8, u32), &Build),
    s: &mut Scratch,
) -> Step<()> {
    s.chosen.clear();
    s.known = None;
    if before.contains(&new) {
        s.chosen.extend(before.iter().map(|id| Scored { distance: 0.0, id: *id }));
        s.known = kept;
        return Ok(());
    }
    src.node(e, base)?;
    s.ids.clear();
    s.ids.extend(before.iter().chain(std::iter::once(&new)));
    src.vectors(&s.ids, &mut s.vecs)?;
    let most = build.most(level);
    let full = before.len() >= most;
    if let Some(k) = kept.filter(|_| full && !build.extend)
        && unkept(metric, (base, before, k), (new, most, build.keep_pruned), s)
    {
        return Ok(());
    }
    measured(metric, base, src.dims(), s);
    if !full {
        // Room again, as another change shrank the list: the new node is
        // added, nothing let go.
        return Ok(());
    }
    select(src, metric, base, (level, build, true), s)
}

/// The heuristic's choice of a full list `before` and `new`, if `new` is
/// one it sets aside, worked out from what the list keeps: `before`'s
/// first `k` were kept, the rest set aside, each nearest `base` first.
/// The heuristic weighs candidates nearest first, each against those kept
/// before it, so one set aside changes no other's lot: the choice is the
/// list with `new` among those set aside, less the farthest of them past
/// the room. So it measures `new` against the kept nearer than it, and
/// the base against as many of the list as place it, not every pair.
/// Whether it was set aside, the choice then in `s.chosen`; `s.vecs` holds
/// `before`'s vectors and then `new`'s.
fn unkept(
    metric: &Metric,
    (base, before, k): (&[f32], &[u32], usize),
    (new, most, keep_pruned): (u32, usize, bool),
    s: &mut Scratch,
) -> bool {
    let dims = base.len();
    let vx = vector_at(&s.vecs, dims, before.len());
    let x = Scored { distance: metric.distance(base, vx), id: new };
    let mut nearer = 0_usize;
    let mut set_aside = false;
    for (at, r) in before.iter().enumerate().take(k) {
        let vr = vector_at(&s.vecs, dims, at);
        if (Scored { distance: metric.distance(base, vr), id: *r }) > x {
            break;
        }
        nearer = nearer.saturating_add(1);
        if metric.distance(vx, vr) <= x.distance {
            set_aside = true;
            break;
        }
    }
    if nearer >= most {
        // The heuristic stops at its most kept before it weighs `new`.
        s.chosen.extend(before.iter().map(|id| Scored { distance: 0.0, id: *id }));
        s.known = Some(k);
        return true;
    }
    if !set_aside {
        return false;
    }
    let room = if keep_pruned { most.saturating_sub(k) } else { 0 };
    let mut aside = before.iter().enumerate().skip(k);
    let mut placed = false;
    s.chosen.extend(before.iter().take(k).map(|id| Scored { distance: 0.0, id: *id }));
    while s.chosen.len() < k.saturating_add(room) {
        let next = if placed { None } else { aside.clone().next() };
        let take_x = match next {
            Some((at, p)) => Scored { distance: metric.distance(base, vector_at(&s.vecs, dims, at)), id: *p } > x,
            None => !placed,
        };
        if take_x {
            placed = true;
            s.chosen.push(x);
        } else if let Some((_, p)) = aside.next() {
            s.chosen.push(Scored { distance: 0.0, id: *p });
        } else {
            break;
        }
    }
    s.known = Some(k);
    true
}
