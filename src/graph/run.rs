//! \[DESIGN\] Running the automaton over the graph [GS §15]: the threads
//! at a vertex, their epsilon closure there, and the steps along each
//! edge from it.
//!
//! A search's state is a vertex and the set of threads at it, so one
//! state stands for every run reading the same letters: the subset
//! construction, made as the search goes. Two runs of one binding thus
//! reach one state, and a search counting paths between states counts
//! bindings, deduplicated as path pattern union requires [P1 §6.5].
//!
//! A restricted scope's threads note the vertex it began at; an edge
//! within it is followed only if the path from there keeps its
//! restrictor [GS §7.1] [P1 Fig. 7]. The whole path's restrictor is
//! checked against bitsets of the path's vertices and edges.

use std::collections::{HashMap, HashSet};

use super::binding::Note;
use super::nfa::{Compiled, Instr, Nfa, Test};
use super::program::{Bound, Restrictor, VarId};
use super::view::{ElementId, GraphView, Hop, LabelId, Orientation};
use crate::error::Result;

mod deferred;

/// Where a restricted scope began, and whether a SIMPLE one has closed
/// its cycle and may follow no more edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Mark {
    pub(super) start: usize,
    pub(super) closed: bool,
}

/// One run of the automaton: its instruction, counters, registers and
/// restricted scopes' marks.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Thread {
    pub(super) pc: usize,
    pub(super) counters: Box<[u32]>,
    pub(super) registers: Box<[Box<[ElementId]>]>,
    pub(super) marks: Box<[Mark]>,
}

/// One step along an edge: the hop, the edge's variable, what is bound
/// at the vertex reached, and the threads there, sorted, each once.
#[derive(Clone, Debug)]
pub(super) struct Successor {
    pub(super) hop: Hop,
    pub(super) var: Option<VarId>,
    pub(super) note: Note,
    pub(super) threads: Vec<Thread>,
}

/// The vertices and edges of the path a restricted search is on, with
/// bitsets of the whole path's, over dense numbers of elements.
#[derive(Debug, Default)]
pub(super) struct Walk {
    pub(super) vertices: Vec<ElementId>,
    pub(super) edges: Vec<ElementId>,
    dense: HashMap<ElementId, usize>,
    seen: Vec<u64>,
}

/// Bits in a word of a bitset.
const WORD_BITS: usize = 64;

/// The bit of dense number `n` in its word.
fn bit(n: usize) -> u64 {
    u32::try_from(n.rem_euclid(WORD_BITS)).ok().and_then(|shift| 1_u64.checked_shl(shift)).unwrap_or(0)
}

impl Walk {
    /// The dense number of `element`.
    fn dense(&mut self, element: ElementId) -> usize {
        let next = self.dense.len();
        *self.dense.entry(element).or_insert(next)
    }

    /// Whether `element` is on the path, by its bit.
    fn on(&self, element: ElementId) -> bool {
        self.dense.get(&element).is_some_and(|&n| self.seen.get(n.div_euclid(WORD_BITS)).is_some_and(|w| w & bit(n) != 0))
    }

    /// Sets or clears the bit of `element`.
    fn flip(&mut self, element: ElementId, on: bool) {
        let n = self.dense(element);
        let word = n.div_euclid(WORD_BITS);
        if self.seen.len() <= word {
            self.seen.resize(word.saturating_add(1), 0);
        }
        if let Some(w) = self.seen.get_mut(word) {
            *w = if on { *w | bit(n) } else { *w & !bit(n) };
        }
    }

    /// A path of `vertex` alone.
    pub(super) fn begin(&mut self, vertex: ElementId) {
        self.vertices.clear();
        self.edges.clear();
        self.seen.iter_mut().for_each(|w| *w = 0);
        self.vertices.push(vertex);
        self.flip(vertex, true);
    }

    /// The path extended by `edge` to `vertex`.
    pub(super) fn push(&mut self, edge: ElementId, vertex: ElementId) {
        self.edges.push(edge);
        self.flip(edge, true);
        // A vertex already on the path is a SIMPLE path's first, back at
        // its end: its bit stays for the earlier place.
        if !self.on(vertex) {
            self.flip(vertex, true);
        }
        self.vertices.push(vertex);
    }

    /// The path without its last step.
    pub(super) fn pop(&mut self) {
        if let (Some(edge), Some(vertex)) = (self.edges.pop(), self.vertices.pop()) {
            self.flip(edge, false);
            if !self.vertices.contains(&vertex) {
                self.flip(vertex, false);
            }
        }
    }

    /// Whether following `edge` to `vertex` keeps the restrictor of a
    /// scope begun at `mark`, and whether a SIMPLE one closes by it.
    fn keeps(&self, restrictor: Restrictor, mark: Mark, (edge, vertex): (ElementId, ElementId)) -> (bool, bool) {
        let whole = mark.start == 0;
        let since = |from: usize| self.vertices.get(from..).unwrap_or_default();
        match restrictor {
            Restrictor::Walk => (true, false),
            Restrictor::Trail => {
                (!if whole { self.on(edge) } else { self.edges.get(mark.start..).unwrap_or_default().contains(&edge) }, false)
            }
            Restrictor::Acyclic => (!if whole { self.on(vertex) } else { since(mark.start).contains(&vertex) }, false),
            Restrictor::Simple => {
                let first = self.vertices.get(mark.start) == Some(&vertex);
                (!since(mark.start.saturating_add(1)).contains(&vertex), first)
            }
        }
    }
}

/// Runs an automaton over a graph.
pub(super) struct Runner<'a> {
    pub(super) nfa: &'a Nfa,
    pub(super) view: &'a dyn GraphView,
    /// The restrictor of each slot.
    restrictors: Vec<Restrictor>,
    labels: Vec<LabelId>,
    /// \[DESIGN\] Closures already taken, by their seeds and the vertex's
    /// labels, when nothing else decides them: an automaton with no
    /// register, counter, restrictor or prefilter.
    memo: Option<HashMap<Closed, Closure>>,
    /// The hops of a frontier's vertices, found at once (`prefetch`).
    fetched: HashMap<(ElementId, Orientation), Vec<Hop>>,
}

/// What a closure of a plain automaton depends on: its seeds and the
/// labels of its vertex, if a vertex test reads them.
type Closed = (Vec<Thread>, Vec<LabelId>);

/// A closure: its threads, grouped by what they bound at the vertex.
type Closure = Vec<(Note, Vec<Thread>)>;

impl<'a> Runner<'a> {
    /// A runner of `nfa` over `view`.
    pub(super) fn new(nfa: &'a Nfa, view: &'a dyn GraphView) -> Runner<'a> {
        let mut restrictors = vec![Restrictor::Walk; nfa.slots];
        for scope in &nfa.scopes {
            if let Some(r) = scope.slot.and_then(|slot| restrictors.get_mut(slot)) {
                *r = scope.restrictor;
            }
        }
        let plain = nfa.plain() && nfa.tests.iter().all(|t| t.filter.is_none()) && nfa.scopes.iter().all(|s| s.filter.is_none());
        Runner { nfa, view, restrictors, labels: Vec::new(), memo: plain.then(HashMap::new), fetched: HashMap::new() }
    }

    /// \[DESIGN\] Finds the hops of each of `asked` at once, a vectorized
    /// Expand (`GraphView::expand`), for the successors taken next; those
    /// found before are forgotten. A vertex's hops are those `hops` gives,
    /// so what follows them does not change.
    ///
    /// # Errors
    ///
    /// Any error of the view.
    pub(super) fn prefetch(&mut self, mut asked: Vec<(ElementId, Orientation)>) -> Result<()> {
        self.fetched.clear();
        asked.retain(|&(_, o)| o != Orientation::NONE);
        asked.sort_unstable_by_key(|&(v, _)| v);
        asked.dedup();
        let mut lists = Vec::with_capacity(asked.len());
        self.view.expand(&asked, &mut lists)?;
        self.fetched.extend(asked.into_iter().zip(lists));
        Ok(())
    }

    /// The thread the automaton starts with.
    fn first(&self) -> Thread {
        let registers = vec![Box::<[ElementId]>::default(); self.nfa.registers].into_boxed_slice();
        let marks = vec![Mark { start: 0, closed: false }; self.nfa.slots].into_boxed_slice();
        Thread { pc: self.nfa.start, counters: vec![0; self.nfa.counters].into_boxed_slice(), registers, marks }
    }

    /// The states a path begun at `vertex` starts in: the threads there,
    /// grouped by what they bind there.
    ///
    /// # Errors
    ///
    /// Any error of the view or a prefilter.
    pub(super) fn start(&mut self, vertex: ElementId) -> Result<Vec<(Note, Vec<Thread>)>> {
        let first = self.first();
        self.closure(vertex, 0, vec![first])
    }

    /// A thread of a plain automaton, at `pc` alone.
    pub(super) fn plain(&self, pc: usize) -> Thread {
        Thread { pc, ..self.first() }
    }

    /// The instructions a plain thread at `pc` reaches at `vertex`: those
    /// waiting to follow an edge, and the end.
    ///
    /// # Errors
    ///
    /// Any error of the view or a prefilter.
    pub(super) fn arrive(&mut self, vertex: ElementId, pc: usize) -> Result<Vec<usize>> {
        let seed = self.plain(pc);
        let mut pcs: Vec<usize> =
            self.closure(vertex, 0, vec![seed])?.into_iter().flat_map(|(_, threads)| threads.into_iter().map(|t| t.pc)).collect();
        pcs.sort_unstable();
        pcs.dedup();
        Ok(pcs)
    }

    /// Where a plain thread at `pc` goes on along `hop`, if it follows it.
    ///
    /// # Errors
    ///
    /// Any error of the view or a prefilter.
    pub(super) fn crosses(&mut self, pc: usize, hop: &Hop) -> Result<Option<usize>> {
        let Some(Instr::Edge { test, orientation, next }) = self.nfa.instr(pc) else { return Ok(None) };
        let (Some(test), next) = (self.nfa.tests.get(*test), *next) else { return Ok(None) };
        let seed = self.plain(pc);
        Ok((orientation.allows(hop.way) && self.passes(test, hop.edge, &seed)?).then_some(next))
    }

    /// Whether any of `threads` has matched the pattern.
    pub(super) fn accepts(&self, threads: &[Thread]) -> bool {
        threads.iter().any(|t| matches!(self.nfa.instr(t.pc), Some(Instr::Accept)))
    }

    /// The ways `threads` may follow edges.
    pub(super) fn orientation(&self, threads: &[Thread]) -> Orientation {
        threads.iter().fold(Orientation::NONE, |o, t| {
            if let Some(Instr::Edge { orientation, .. }) = self.nfa.instr(t.pc) { o.union(*orientation) } else { o }
        })
    }

    /// Whether `test` passes `element`, given `thread`'s registers.
    fn passes(&mut self, test: &Test, element: ElementId, thread: &Thread) -> Result<bool> {
        if let Some(labels) = &test.labels {
            self.labels.clear();
            self.view.labels(element, &mut self.labels)?;
            if !labels.holds(&self.labels) {
                return Ok(false);
            }
        }
        match test.filter.as_ref().filter(|_| test.defer.is_none()) {
            Some(filter) => filtered(filter, Some((test.var, element)), thread),
            None => Ok(true),
        }
    }

    /// The steps from `vertex`, `position` edges into the path, of
    /// `threads`: along each edge, grouped by the edge's variable and what
    /// is bound at the vertex reached, in the order of the edges.
    ///
    /// # Errors
    ///
    /// Any error of the view or a prefilter.
    pub(super) fn successors(
        &mut self,
        vertex: ElementId,
        (position, walk): (usize, Option<&Walk>),
        threads: &[Thread],
        out: &mut Vec<Successor>,
    ) -> Result<()> {
        let orientation = self.orientation(threads);
        if orientation == Orientation::NONE {
            return Ok(());
        }
        let mut hops = self.fetched.get(&(vertex, orientation)).cloned().unwrap_or_default();
        if !self.fetched.contains_key(&(vertex, orientation)) {
            self.view.hops(vertex, orientation, &mut hops)?;
        }
        hops.sort_unstable_by_key(|h| (h.edge, h.vertex, h.way));
        let mut at = 0;
        while let Some(&hop) = hops.get(at) {
            // Hops of one edge to one vertex, in different ways, are one
            // letter: their threads go on together.
            let same = hops.get(at..).unwrap_or_default().iter().take_while(|h| (h.edge, h.vertex) == (hop.edge, hop.vertex)).count();
            let ways: Vec<Hop> = hops.get(at..at.saturating_add(same)).unwrap_or_default().to_vec();
            at = at.saturating_add(same.max(1));
            self.along(&ways, (position, walk), threads, out)?;
        }
        Ok(())
    }

    /// The steps of `threads` along one edge, followed in each of `ways`.
    pub(super) fn along(
        &mut self,
        ways: &[Hop],
        (position, walk): (usize, Option<&Walk>),
        threads: &[Thread],
        out: &mut Vec<Successor>,
    ) -> Result<()> {
        let mut buckets: Vec<(Option<VarId>, Vec<Thread>)> = Vec::new();
        for hop in ways {
            for thread in threads {
                let Some(Instr::Edge { test, orientation, next }) = self.nfa.instr(thread.pc) else { continue };
                let Some(test) = self.nfa.tests.get(*test) else { continue };
                if !orientation.allows(hop.way) || !self.passes(test, hop.edge, thread)? {
                    continue;
                }
                let Some(moved) = self.moved(test, thread, (hop, *next), walk) else { continue };
                let Some(moved) = self.triggered(test, moved)? else { continue };
                match buckets.iter_mut().find(|b| b.0 == test.var) {
                    Some(bucket) => bucket.1.push(moved),
                    None => buckets.push((test.var, vec![moved])),
                }
            }
        }
        let Some(hop) = ways.first() else { return Ok(()) };
        for (var, moved) in buckets {
            for (note, reached) in self.closure(hop.vertex, position.saturating_add(1), moved)? {
                out.push(Successor { hop: *hop, var, note, threads: reached });
            }
        }
        Ok(())
    }

    /// `thread` along `hop` to `next`, its edge bound, if its joins and
    /// its restricted scopes allow.
    fn moved(&self, test: &Test, thread: &Thread, (hop, next): (&Hop, usize), walk: Option<&Walk>) -> Option<Thread> {
        let mut moved = self.bound(test, thread, hop.edge)?;
        for &slot in &test.slots {
            let (mark, restrictor) = (*moved.marks.get(slot)?, *self.restrictors.get(slot)?);
            if mark.closed {
                return None;
            }
            let (keeps, closes) = walk.map_or((true, false), |w| w.keeps(restrictor, mark, (hop.edge, hop.vertex)));
            if !keeps {
                return None;
            }
            if let Some(m) = moved.marks.get_mut(slot) {
                m.closed = closes;
            }
        }
        moved.pc = next;
        Some(moved)
    }

    /// The epsilon closure at `vertex`, `position` edges into the path,
    /// of `seeds`: the threads waiting to follow an edge or done, grouped
    /// by what they bound at the vertex and the branches they took.
    fn closure(&mut self, vertex: ElementId, position: usize, mut seeds: Vec<Thread>) -> Result<Vec<(Note, Vec<Thread>)>> {
        if self.memo.is_none() {
            return self.close(vertex, position, seeds);
        }
        seeds.sort_unstable();
        seeds.dedup();
        let mut labels = Vec::new();
        if self.nfa.tests.iter().any(|t| t.labels.is_some()) {
            self.view.labels(vertex, &mut labels)?;
            labels.sort_unstable();
        }
        let key = (seeds, labels);
        if let Some(done) = self.memo.as_ref().and_then(|m| m.get(&key)) {
            return Ok(done.clone());
        }
        let done = self.close(vertex, position, key.0.clone())?;
        if let Some(memo) = self.memo.as_mut() {
            memo.insert(key, done.clone());
        }
        Ok(done)
    }

    /// The closure itself, as `closure` gives it.
    fn close(&mut self, vertex: ElementId, position: usize, seeds: Vec<Thread>) -> Result<Vec<(Note, Vec<Thread>)>> {
        let mut stack: Vec<(Thread, Note)> = seeds.into_iter().map(|t| (t, Note::default())).collect();
        let mut seen: HashSet<(Thread, Note)> = HashSet::new();
        let mut done: Vec<(Note, Vec<Thread>)> = Vec::new();
        while let Some((thread, note)) = stack.pop() {
            let firsts = self.nfa.closure.get(thread.pc).cloned().unwrap_or_default();
            for pc in firsts.into_iter().rev() {
                let mut at = thread.clone();
                at.pc = pc;
                if !seen.insert((at.clone(), note.clone())) {
                    continue;
                }
                match self.nfa.instr(pc) {
                    Some(Instr::Accept) => self.accepted(&mut done, &note, at)?,
                    Some(Instr::Edge { .. }) => gather(&mut done, &note, at),
                    Some(instr) => stack.extend(self.step(&instr.clone(), (vertex, position), at, &note)?),
                    None => {}
                }
            }
        }
        for group in &mut done {
            group.1.sort_unstable();
            group.1.dedup();
        }
        done.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        Ok(done)
    }

    /// `at` at Accept gathered into `done` of `note`, if its deferred
    /// prefilters hold.
    fn accepted(&self, done: &mut Vec<(Note, Vec<Thread>)>, note: &Note, mut at: Thread) -> Result<()> {
        if self.settled(&mut at)? {
            gather(done, note, at);
        }
        Ok(())
    }

    /// `thread` entering `scope`, then at `next`: none if a deferred
    /// prefilter it flushes fails.
    fn entered(&self, (scope, next): (usize, usize), position: usize, mut thread: Thread, note: Note) -> Result<Vec<(Thread, Note)>> {
        if !self.flushed(scope, &mut thread)? {
            return Ok(Vec::new());
        }
        self.enter(scope, position, &mut thread);
        thread.pc = next;
        Ok(vec![(thread, note)])
    }

    /// The threads one instruction other than an edge's or a fork's
    /// makes of `thread` at `vertex`.
    fn step(
        &mut self,
        instr: &Instr,
        (vertex, position): (ElementId, usize),
        mut thread: Thread,
        note: &Note,
    ) -> Result<Vec<(Thread, Note)>> {
        let mut note = note.clone();
        match *instr {
            Instr::Vertex { test, next } => {
                let Some(test) = self.nfa.tests.get(test) else { return Ok(Vec::new()) };
                if !self.passes(test, vertex, &thread)? {
                    return Ok(Vec::new());
                }
                let Some(bound) = self.bound(test, &thread, vertex) else { return Ok(Vec::new()) };
                let Some(mut bound) = self.triggered(test, bound)? else { return Ok(Vec::new()) };
                bound.pc = next;
                note.vars.extend(test.var);
                note.vars.sort_unstable();
                Ok(vec![(bound, note)])
            }
            Instr::Tag { tag, next } => {
                note.tags.push(tag);
                thread.pc = next;
                Ok(vec![(thread, note)])
            }
            Instr::Enter { scope, next } => self.entered((scope, next), position, thread, note),
            Instr::Leave { scope, next } => {
                let filter = self.nfa.scopes.get(scope).and_then(|s| s.filter.as_ref());
                if !filter.map_or(Ok(true), |f| filtered(f, None, &thread))? {
                    return Ok(Vec::new());
                }
                thread.pc = next;
                Ok(vec![(thread, note)])
            }
            Instr::Init { counter, next } => {
                if let Some(c) = thread.counters.get_mut(counter) {
                    *c = 0;
                }
                thread.pc = next;
                Ok(vec![(thread, note)])
            }
            Instr::Loop { counter, min, max, body, exit } => Ok(looped(thread, (counter, min, max), (body, exit), &note)),
            Instr::Count { counter, cap, next } => {
                if let Some(c) = thread.counters.get_mut(counter) {
                    *c = c.saturating_add(1).min(cap);
                }
                thread.pc = next;
                Ok(vec![(thread, note)])
            }
            Instr::Edge { .. } | Instr::Split(_) | Instr::Accept => Ok(Vec::new()),
        }
    }

    /// Scope `scope` begins `position` edges into the path: its PREV
    /// registers set, its FIRST registers if not yet, its registers
    /// cleared, its start noted.
    fn enter(&self, scope: usize, position: usize, thread: &mut Thread) {
        let Some(scope) = self.nfa.scopes.get(scope) else { return };
        for &(previous, source) in &scope.previous {
            let last: Box<[ElementId]> = thread.registers.get(source).and_then(|r| r.last()).copied().into_iter().collect();
            if let Some(r) = thread.registers.get_mut(previous) {
                *r = last;
            }
        }
        for &(first, source) in &scope.first {
            let last: Box<[ElementId]> = thread.registers.get(source).and_then(|r| r.last()).copied().into_iter().collect();
            if let Some(r) = thread.registers.get_mut(first).filter(|r| r.is_empty()) {
                *r = last;
            }
        }
        for &register in &scope.clears {
            if let Some(r) = thread.registers.get_mut(register) {
                *r = Box::default();
            }
        }
        if let Some(mark) = scope.slot.and_then(|slot| thread.marks.get_mut(slot)) {
            *mark = Mark { start: position, closed: false };
        }
    }
}

/// The threads between iterations of a counted quantifier: another
/// iteration while fewer than `max` are done, out once `min` are.
fn looped(
    thread: Thread,
    (counter, min, max): (usize, u32, Option<u32>),
    (body, exit): (usize, usize),
    note: &Note,
) -> Vec<(Thread, Note)> {
    let count = thread.counters.get(counter).copied().unwrap_or_default();
    let mut out = Vec::new();
    if max.is_none_or(|max| count < max) {
        let mut again = thread.clone();
        again.pc = body;
        out.push((again, note.clone()));
    }
    if count >= min {
        let mut exit_thread = thread;
        exit_thread.pc = exit;
        out.push((exit_thread, note.clone()));
    }
    out
}

/// `thread` with `element` bound by `test`, none if a join fails
/// [GS §6.4]: a joining register's later binding must equal its first.
fn bind(test: &Test, thread: &Thread, element: ElementId) -> Option<Thread> {
    let mut bound = thread.clone();
    for &(register, joins) in &test.binds {
        let held = bound.registers.get_mut(register)?;
        if let (true, Some(first)) = (joins, held.first()) {
            if *first != element {
                return None;
            }
            continue;
        }
        let mut list = held.to_vec();
        list.push(element);
        *held = list.into_boxed_slice();
    }
    Some(bound)
}

/// Whether `filter` holds of `element`, if an element pattern's, given
/// `thread`'s registers.
fn filtered(filter: &Compiled, element: Option<(Option<VarId>, ElementId)>, thread: &Thread) -> Result<bool> {
    let path = filter.path.and_then(|r| thread.registers.get(r)).map_or(&[][..], |p| &**p);
    (filter.test)(&Bound { element, registers: &thread.registers, reads: &filter.reads, path })
}

/// Adds `thread` to the group of `done` of `note`.
fn gather(done: &mut Vec<(Note, Vec<Thread>)>, note: &Note, thread: Thread) {
    match done.iter_mut().find(|d| &d.0 == note) {
        Some(group) => group.1.push(thread),
        None => done.push((note.clone(), vec![thread])),
    }
}
