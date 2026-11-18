//! \[DESIGN\] Searches of paths [GS §15]: where a restrictor applies, a
//! state is a path, not a vertex and threads, since what may follow
//! depends on what the path has passed.
//!
//! PathSearch-DFS enumerates every path, for no selector or ANY k, or a
//! walk of bounded quantifiers alone, depth first with the path's
//! vertices and edges in bitsets, giving each path as it is found. A
//! selector by length or cost takes paths best first, shortest or
//! cheapest first, so its partitions fill in order and the search stops
//! once the ends given are complete: "search shortest first, verify the
//! restrictor, continue if violated" [GS §15 notes]. Both are exponential
//! at worst [GS §15 notes]; the limits bound them [GS §20].

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};

use super::ranked::{Cost, edge_cost};
use super::selection::Selection;
use super::states::{Context, STATE_BYTES, THREAD_BYTES, index};
use crate::error::Result;
use crate::graph::binding::{Letter, Notes, PathBinding};
use crate::graph::limits::Meter;
use crate::graph::program::Selector;
use crate::graph::run::{Runner, Thread, Walk};
use crate::graph::view::{ElementId, Hop};

/// A step a frame may take: its hop, none for a start, letter and threads.
type Child = (Option<Hop>, Letter, Vec<Thread>);

/// A path's step on a depth-first search's stack: its letter and
/// threads, and its children once found.
struct Frame {
    letter: Option<Letter>,
    threads: Vec<Thread>,
    children: Option<Vec<Child>>,
    next: usize,
}

/// A depth-first search of paths from each start in turn, giving each
/// path found.
pub(crate) struct Dfs<'s> {
    ctx: Context<'s>,
    runner: Runner<'s>,
    selector: Selector,
    walk: Walk,
    notes: Notes,
    frames: Vec<Frame>,
    starts: VecDeque<ElementId>,
    start: Option<ElementId>,
    /// The paths given of each partition.
    counts: HashMap<(ElementId, ElementId), usize>,
    pub(super) meter: Meter,
}

impl<'s> Dfs<'s> {
    /// A search from each of `starts`, keeping what `selector` keeps.
    pub(super) fn new(ctx: Context<'s>, selector: Selector, starts: Vec<ElementId>, meter: Meter) -> Dfs<'s> {
        let runner = Runner::new(ctx.nfa, ctx.view);
        let (walk, notes, frames, counts) = (Walk::default(), Notes::default(), Vec::new(), HashMap::new());
        Dfs { ctx, runner, selector, walk, notes, frames, starts: starts.into(), start: None, counts, meter }
    }

    /// The next path, none once every start is searched.
    ///
    /// # Errors
    ///
    /// Any error of the view, a prefilter or a limit.
    pub(crate) fn next_path(&mut self) -> Result<Option<PathBinding>> {
        loop {
            if self.frames.is_empty() && !self.begin()? {
                return Ok(None);
            }
            if let Some(found) = self.advance()? {
                return Ok(Some(found));
            }
        }
    }

    /// Begins the search of the next start: false if none is left.
    fn begin(&mut self) -> Result<bool> {
        let Some(start) = self.starts.pop_front() else { return Ok(false) };
        self.start = Some(start);
        self.walk.begin(start);
        let mut children = Vec::new();
        for (note, threads) in self.runner.start(start)? {
            children.push((None, Letter { edge: None, vertex: start, note: self.notes.id(note) }, threads));
        }
        self.frames.push(Frame { letter: None, threads: Vec::new(), children: Some(children), next: 0 });
        Ok(true)
    }

    /// One move of the search: into the next child of the top frame, or
    /// out of it; a path if one is found.
    fn advance(&mut self) -> Result<Option<PathBinding>> {
        self.meter.check()?;
        let Some(top) = self.frames.last_mut() else { return Ok(None) };
        if top.children.is_none() {
            let depth = self.walk.edges.len();
            let mut found = Vec::new();
            self.runner.successors(
                self.walk.vertices.last().copied().unwrap_or(ElementId::new(0, 0)),
                (depth, Some(&self.walk)),
                &top.threads,
                &mut found,
            )?;
            let deeper = found.is_empty() || self.meter.length(depth.saturating_add(1))?;
            let children = found.into_iter().filter(|_| deeper).map(|s| {
                let letter = Letter { edge: Some((s.hop.edge, s.var)), vertex: s.hop.vertex, note: self.notes.id(s.note) };
                (Some(s.hop), letter, s.threads)
            });
            top.children = Some(children.collect());
        }
        let child = top.children.as_mut().and_then(|c| c.get_mut(top.next)).map(|c| (c.0, c.1, std::mem::take(&mut c.2)));
        top.next = top.next.saturating_add(1);
        let Some((hop, letter, threads)) = child else {
            if self.frames.pop().is_some_and(|f| f.letter.is_some_and(|l| l.edge.is_some())) {
                self.walk.pop();
            }
            return Ok(None);
        };
        if let Some(hop) = hop {
            self.walk.push(hop.edge, hop.vertex);
        }
        let bytes = STATE_BYTES.saturating_add(threads.len().saturating_mul(THREAD_BYTES));
        if !self.meter.state(bytes)? {
            self.frames.clear();
            self.starts.clear();
            return Ok(None);
        }
        let accepting = self.runner.accepts(&threads);
        self.frames.push(Frame { letter: Some(letter), threads, children: None, next: 0 });
        if accepting { self.found(letter.vertex) } else { Ok(None) }
    }

    /// The path on the stack, ending at `end`, if its partition keeps it.
    fn found(&mut self, end: ElementId) -> Result<Option<PathBinding>> {
        let Some(start) = self.start.filter(|_| self.ctx.ends_at(end)) else { return Ok(None) };
        let count = self.counts.entry((start, end)).or_insert(0);
        let k = match self.selector {
            Selector::Any(k) => usize::try_from(k).unwrap_or(usize::MAX),
            Selector::All
            | Selector::AnyShortest
            | Selector::AllShortest
            | Selector::Shortest(_)
            | Selector::ShortestGroups(_)
            | Selector::AnyCheapest
            | Selector::AllCheapest
            | Selector::Cheapest(_) => usize::MAX,
        };
        if *count >= k {
            return Ok(None);
        }
        *count = count.saturating_add(1);
        let total = *count;
        if !self.meter.paths(total)? {
            return Ok(None);
        }
        let letters: Vec<Letter> = self.frames.iter().filter_map(|f| f.letter).collect();
        Ok(Some(PathBinding::of(&letters, &self.notes)))
    }
}

/// A path of a best-first search: the path it extends, its letter,
/// hop and threads.
struct Node {
    parent: Option<u32>,
    letter: Letter,
    hop: Option<Hop>,
    threads: Vec<Thread>,
}

/// The paths from `start` a selector by length or cost keeps, found best first.
///
/// # Errors
///
/// Any error of the view, a prefilter, a cost or a limit.
pub(super) fn best_first(ctx: &Context<'_>, start: ElementId, selector: Selector, meter: &mut Meter) -> Result<Vec<PathBinding>> {
    let mut runner = Runner::new(ctx.nfa, ctx.view);
    let (mut notes, mut queue, mut walk) = (Notes::default(), BinaryHeap::new(), Walk::default());
    let mut tree: Vec<Node> = Vec::new();
    let mut selection = Selection::new(selector);
    for (note, threads) in runner.start(start)? {
        let letter = Letter { edge: None, vertex: start, note: notes.id(note) };
        queue.push(Reverse((Cost(0.0), u32::try_from(tree.len()).unwrap_or(u32::MAX))));
        tree.push(Node { parent: None, letter, hop: None, threads });
    }
    while let Some(Reverse((Cost(cost), at))) = queue.pop() {
        meter.check()?;
        if selection.closed(ctx, cost) {
            break;
        }
        let path = chain(&tree, at);
        rewalk(&mut walk, start, &path, &tree);
        let Some(node) = tree.get(index(at)) else { continue };
        if runner.accepts(&node.threads) && ctx.ends_at(node.letter.vertex) {
            let letters: Vec<Letter> = path.iter().filter_map(|&n| tree.get(index(n)).map(|x| x.letter)).collect();
            let mut binding = PathBinding::of(&letters, &notes);
            binding.cost = ctx.by_cost().then_some(cost);
            if !selection.offer(binding, cost, meter)? {
                break;
            }
        }
        let mut successors = Vec::new();
        runner.successors(node.letter.vertex, (walk.edges.len(), Some(&walk)), &node.threads, &mut successors)?;
        if !successors.is_empty() && !meter.length(walk.edges.len().saturating_add(1))? {
            break;
        }
        for s in successors {
            let step = cost + edge_cost(ctx, s.hop.edge)?;
            let letter = Letter { edge: Some((s.hop.edge, s.var)), vertex: s.hop.vertex, note: notes.id(s.note) };
            if !meter.state(STATE_BYTES.saturating_add(s.threads.len().saturating_mul(THREAD_BYTES)))? {
                return Ok(selection.bindings());
            }
            queue.push(Reverse((Cost(step), u32::try_from(tree.len()).unwrap_or(u32::MAX))));
            tree.push(Node { parent: Some(at), letter, hop: Some(s.hop), threads: s.threads });
        }
    }
    Ok(selection.bindings())
}

/// The tree of the path ending at `at`, first to last.
fn chain(tree: &[Node], at: u32) -> Vec<u32> {
    let mut path = vec![at];
    while let Some(parent) = path.last().and_then(|&n| tree.get(index(n))).and_then(|n| n.parent) {
        path.push(parent);
    }
    path.reverse();
    path
}

/// Makes `walk` the path of `path`'s tree from `start`.
fn rewalk(walk: &mut Walk, start: ElementId, path: &[u32], tree: &[Node]) {
    walk.begin(start);
    for hop in path.iter().filter_map(|&n| tree.get(index(n)).and_then(|x| x.hop)) {
        walk.push(hop.edge, hop.vertex);
    }
}
