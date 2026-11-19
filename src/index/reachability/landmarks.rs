//! \[DESIGN\] Pruned landmark labelling for reachability (RULES.md D150;
//! Yano, Akiba, Iwata and Yoshida, "Fast and Scalable Reachability Queries
//! on Graphs by Pruned Labeling with Landmarks and Paths", CIKM 2013):
//! each vertex has an out label, the landmarks it reaches, and an in
//! label, those reaching it, so `s` reaches `t` if and only if `s` is `t`
//! or their labels share a landmark. Landmarks are numbered by rank, the
//! vertices ordered by the product of their degrees, highest first, and
//! a label lists ranks in ascending order, so two are intersected by one
//! merge.
//!
//! It is made of every edge at once (`made`): from each vertex in rank
//! order, a breadth-first search forward and one backward, pruned at a
//! vertex whose pair the labels already answer. An edge `a -> b` added
//! after (`insert`) changes nothing if `a` reached `b`; otherwise `a`
//! becomes the landmark of every pair it joins: a search backward from
//! `a` adds it to the out labels of what reaches `a`, one forward from
//! `b` to the in labels of what `b` reaches, each pruned at a vertex
//! that reached `b`, or that `a` reached, before the edge. The labels are
//! exact while edges are only added: the backward search runs first, so
//! each test reads the labels as they were before the edge. A removal
//! leaves them claiming pairs no longer joined, so the index drops them
//! until maintenance makes them again (`reachability.rs`).

use std::collections::{HashMap, HashSet, VecDeque};

/// \[DESIGN\] Bytes a vertex is counted at beside its key: its number,
/// its rank, its share of a hash table, and four lists' headers.
const VERTEX_BYTES: usize = 160;
/// Bytes of an edge, in the lists of both its ends.
const EDGE_BYTES: usize = 8;
/// Bytes of a landmark in a label.
const LANDMARK_BYTES: usize = 4;

/// Whether two ascending lists share an element.
fn shared(a: &[u32], b: &[u32]) -> bool {
    let (mut i, mut j) = (a.iter().peekable(), b.iter().peekable());
    while let (Some(&&x), Some(&&y)) = (i.peek(), j.peek()) {
        if x == y {
            return true;
        }
        if x < y {
            i.next();
        } else {
            j.next();
        }
    }
    false
}

/// Adds `rank` to the ascending list `label`, if it is not in it: whether it was added.
fn put(label: &mut Vec<u32>, rank: u32) -> bool {
    match label.binary_search(&rank) {
        Ok(_) => false,
        Err(at) => {
            label.insert(at, rank);
            true
        }
    }
}

/// The labels of an edge table's vertices.
#[derive(Default)]
pub(super) struct Landmarks {
    /// Each vertex's number, by its key's bytes.
    ids: HashMap<Vec<u8>, u32>,
    /// Of each vertex, the vertices its edges lead to, and come from.
    out: Vec<Vec<u32>>,
    into: Vec<Vec<u32>>,
    /// Of each vertex, its rank.
    rank: Vec<u32>,
    /// Of each vertex, the ranks of the landmarks it reaches, and that reach it.
    out_label: Vec<Vec<u32>>,
    in_label: Vec<Vec<u32>>,
    /// The bytes it is counted at.
    bytes: usize,
}

/// Which way a search follows edges.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Way {
    /// From source to destination, adding to in labels.
    Forward,
    /// From destination to source, adding to out labels.
    Backward,
}

impl Landmarks {
    /// The number of `key`, given if it has none: ranked after every other.
    fn id(&mut self, key: &[u8]) -> u32 {
        if let Some(&id) = self.ids.get(key) {
            return id;
        }
        let id = u32::try_from(self.rank.len()).unwrap_or(u32::MAX);
        self.ids.insert(key.to_vec(), id);
        self.rank.push(id);
        for list in [&mut self.out, &mut self.into, &mut self.out_label, &mut self.in_label] {
            list.push(Vec::new());
        }
        self.bytes = self.bytes.saturating_add(VERTEX_BYTES.saturating_add(key.len()));
        id
    }

    /// The bytes it is counted at.
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The vertices of number `id` leads to, or comes from.
    fn next(&self, id: u32, way: Way) -> &[u32] {
        let lists = if way == Way::Forward { &self.out } else { &self.into };
        lists.get(usize::try_from(id).unwrap_or(usize::MAX)).map_or(&[], Vec::as_slice)
    }

    /// The out label, or the in label, of `id`.
    fn label(&self, id: u32, out: bool) -> &[u32] {
        let labels = if out { &self.out_label } else { &self.in_label };
        labels.get(usize::try_from(id).unwrap_or(usize::MAX)).map_or(&[], Vec::as_slice)
    }

    /// Adds landmark `rank` to `id`'s label of `way`: whether it was added.
    fn mark(&mut self, id: u32, way: Way, rank: u32) -> bool {
        let labels = if way == Way::Forward { &mut self.in_label } else { &mut self.out_label };
        let added = labels.get_mut(usize::try_from(id).unwrap_or(usize::MAX)).is_some_and(|l| put(l, rank));
        if added {
            self.bytes = self.bytes.saturating_add(LANDMARK_BYTES);
        }
        added
    }

    /// Whether `s` reaches `t`, by their numbers, of the labels alone.
    fn labelled(&self, s: u32, t: u32) -> bool {
        shared(self.label(s, true), self.label(t, false))
    }

    /// Whether `s` reaches `t`, by their numbers.
    pub(super) fn reaches(&self, s: u32, t: u32) -> bool {
        s == t || self.labelled(s, t)
    }

    /// Adds the edge from `a` to `b`, by their numbers, to the lists of both.
    fn link(&mut self, a: u32, b: u32) {
        let at = |id: u32| usize::try_from(id).unwrap_or(usize::MAX);
        let fresh = self.out.get(at(a)).is_some_and(|l| !l.contains(&b));
        if fresh {
            if let Some(l) = self.out.get_mut(at(a)) {
                l.push(b);
            }
            if let Some(l) = self.into.get_mut(at(b)) {
                l.push(a);
            }
            self.bytes = self.bytes.saturating_add(EDGE_BYTES);
        }
    }

    /// The labels of the graph of `edges`, each the keys of its source
    /// and destination.
    pub(super) fn made(edges: &[(Vec<u8>, Vec<u8>)]) -> Landmarks {
        let mut made = Landmarks::default();
        for (s, d) in edges {
            let (a, b) = (made.id(s), made.id(d));
            made.link(a, b);
        }
        let mut order: Vec<u32> = (0..u32::try_from(made.rank.len()).unwrap_or(u32::MAX)).collect();
        let weight = |id: u32| {
            let degree = |way| u64::try_from(made.next(id, way).len()).unwrap_or(u64::MAX).saturating_add(1);
            degree(Way::Forward).saturating_mul(degree(Way::Backward))
        };
        order.sort_by_key(|&id| (std::cmp::Reverse(weight(id)), id));
        for (rank, &id) in (0_u32..).zip(&order) {
            if let Some(r) = made.rank.get_mut(usize::try_from(id).unwrap_or(usize::MAX)) {
                *r = rank;
            }
        }
        for (rank, &id) in (0_u32..).zip(&order) {
            made.pruned(id, rank, Way::Forward);
            made.pruned(id, rank, Way::Backward);
        }
        made
    }

    /// The pruned search of the static labelling from landmark `root` of
    /// rank `rank`, following `way`: each vertex not yet answered gets the
    /// landmark, and is searched on from.
    fn pruned(&mut self, root: u32, rank: u32, way: Way) {
        let mut seen = HashSet::from([root]);
        let mut queue = VecDeque::from([root]);
        while let Some(w) = queue.pop_front() {
            let answered = if way == Way::Forward { self.labelled(root, w) } else { self.labelled(w, root) };
            if answered {
                continue;
            }
            self.mark(w, way, rank);
            let next: Vec<u32> = self.next(w, way).iter().copied().filter(|n| seen.insert(*n)).collect();
            queue.extend(next);
        }
    }

    /// The search from `from` after an edge `a -> b` is added, adding
    /// `rank`, `a`'s, to the labels of `way` of each vertex not pruned:
    /// backward, one that reached `b` before; forward, one `a` reached.
    fn spread(&mut self, (a, b): (u32, u32), from: u32, way: Way) {
        let rank = self.rank.get(usize::try_from(a).unwrap_or(usize::MAX)).copied().unwrap_or(u32::MAX);
        let mut seen = HashSet::from([from]);
        let mut queue = VecDeque::from([from]);
        while let Some(w) = queue.pop_front() {
            for n in self.next(w, way).to_vec() {
                if !seen.insert(n) {
                    continue;
                }
                let before = if way == Way::Forward { self.reaches(a, n) } else { self.reaches(n, b) };
                if !before {
                    self.mark(n, way, rank);
                    queue.push_back(n);
                }
            }
        }
    }

    /// Adds the edge from the vertex of key `source` to that of
    /// `destination`, its labels kept exact.
    pub(super) fn insert(&mut self, source: &[u8], destination: &[u8]) {
        let (a, b) = (self.id(source), self.id(destination));
        let before = self.reaches(a, b);
        self.link(a, b);
        if before {
            return;
        }
        let rank = self.rank.get(usize::try_from(a).unwrap_or(usize::MAX)).copied().unwrap_or(u32::MAX);
        // The pairs joined are of what reaches `a` and what `b` reaches.
        self.mark(a, Way::Backward, rank);
        self.spread((a, b), a, Way::Backward);
        self.mark(b, Way::Forward, rank);
        self.spread((a, b), b, Way::Forward);
    }

    /// Of each of `starts`, whether it reaches one of `ends`, following
    /// edges backward if `reverse`, each vertex by its key's bytes; a start
    /// of no key known reaches.
    pub(super) fn reaching(&self, starts: &[Option<Vec<u8>>], ends: &[Vec<u8>], reverse: bool) -> Vec<bool> {
        let named: HashSet<&[u8]> = ends.iter().map(Vec::as_slice).collect();
        let mut hubs: Vec<u32> = ends.iter().filter_map(|e| self.ids.get(e)).flat_map(|&e| self.label(e, reverse).to_vec()).collect();
        hubs.sort_unstable();
        hubs.dedup();
        starts
            .iter()
            .map(|start| {
                let Some(key) = start else { return true };
                named.contains(key.as_slice()) || self.ids.get(key).is_some_and(|&s| shared(self.label(s, !reverse), &hubs))
            })
            .collect()
    }
}
