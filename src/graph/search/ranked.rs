//! \[DESIGN\] PathSearch-KShort and PathSearch-Dijkstra [GS §15]: SHORTEST
//! k, ANY k, CHEAPEST k, ANY CHEAPEST and ALL CHEAPEST by a best-first
//! search of walks, Dijkstra's order on the product of the graph and the
//! automaton, each state taken from the queue at most k times: the
//! k-shortest-walks method of Eppstein's line. The i-th cheapest walk to
//! an end extends one of the k cheapest to each state on it, or the end
//! would have k cheaper walks, so the first k taken at an end are its k
//! cheapest. Lengths are costs of 1 an edge.
//!
//! ALL CHEAPEST takes a state again only at the cost it was first taken
//! at, and no walk through a state twice at one cost: a cycle of cost 0
//! would otherwise give endless walks of the least cost.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};

use super::states::{Context, Costs, States, index, walk_to};
use crate::error::Result;
use crate::graph::binding::{Letter, PathBinding};
use crate::graph::limits::{Meter, cost_error};
use crate::graph::program::Selector;
use crate::graph::run::Runner;
use crate::graph::view::ElementId;

/// A cost, ordered totally.
#[derive(Clone, Copy, Debug)]
pub(super) struct Cost(pub(super) f64);

impl PartialEq for Cost {
    fn eq(&self, other: &Cost) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Cost {}

impl PartialOrd for Cost {
    fn partial_cmp(&self, other: &Cost) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cost {
    fn cmp(&self, other: &Cost) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// The cost of following `edge` in `ctx`, 1 if the search is by length.
///
/// # Errors
///
/// GRF-0034 if it is negative or not a number [GS §7.2].
pub(super) fn edge_cost(ctx: &Context<'_>, edge: ElementId) -> Result<f64> {
    let c = match ctx.cost {
        Costs::Length => return Ok(1.0),
        Costs::Hook(cost) => cost(edge)?,
        Costs::View => ctx.view.cost(edge)?,
    };
    if c.is_finite() && c >= 0.0 { Ok(c) } else { Err(cost_error(c)) }
}

/// One walk in the queue: its state, the entry it extends, its letter
/// and cost.
struct Entry {
    node: u32,
    parent: Option<u32>,
    cost: f64,
    length: usize,
}

/// The search's entries, queue and ends.
struct Ranked<'c, 's> {
    ctx: &'c Context<'s>,
    selector: Selector,
    k: usize,
    states: States,
    entries: Vec<Entry>,
    /// Each entry's parent and letter, for reading walks back.
    parents: Vec<(Option<u32>, Letter)>,
    queue: BinaryHeap<Reverse<(Cost, u32)>>,
    /// How often each state was taken, and at what cost first.
    taken: HashMap<u32, (usize, f64)>,
    ends: Vec<(ElementId, Vec<u32>, f64)>,
}

impl Ranked<'_, '_> {
    /// Queues the walk of entry `parent` and `letter` to the state of `threads`.
    fn push(
        &mut self,
        (parent, letter): (Option<u32>, Letter),
        threads: Vec<crate::graph::run::Thread>,
        cost: f64,
        meter: &mut Meter,
    ) -> Result<bool> {
        let (node, _) = self.states.node(letter.vertex, threads);
        if self.selector == Selector::AllCheapest && self.repeats(parent, node, cost) {
            return Ok(true);
        }
        let entry = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
        let length = parent.and_then(|p| self.entries.get(index(p))).map_or(0, |p| p.length.saturating_add(1));
        self.entries.push(Entry { node, parent, cost, length });
        self.parents.push((parent, letter));
        self.queue.push(Reverse((Cost(cost), entry)));
        meter.state(self.states.bytes(node))
    }

    /// Whether the walk of entry `parent` passes `node` at `cost` already.
    fn repeats(&self, parent: Option<u32>, node: u32, cost: f64) -> bool {
        let mut at = parent;
        while let Some(entry) = at.and_then(|e| self.entries.get(index(e))) {
            if entry.node == node && Cost(entry.cost) == Cost(cost) {
                return true;
            }
            at = entry.parent;
        }
        false
    }

    /// Whether entry `entry`, taken from the queue, is expanded: its state
    /// taken fewer than k times, or for ALL CHEAPEST at its least cost.
    fn take(&mut self, node: u32, cost: f64) -> bool {
        let (k, all) = (self.k, self.selector == Selector::AllCheapest);
        let taken = self.taken.entry(node).or_insert((0, cost));
        let fresh = if all { Cost(taken.1) == Cost(cost) } else { taken.0 < k };
        if fresh {
            taken.0 = taken.0.saturating_add(1);
        }
        fresh
    }

    /// Notes entry `entry`, accepting at `vertex` at `cost`, if its end
    /// keeps it.
    fn accept(&mut self, vertex: ElementId, entry: u32, cost: f64) {
        if !self.ctx.ends_at(vertex) {
            return;
        }
        let (k, all) = (self.k, self.selector == Selector::AllCheapest);
        match self.ends.iter_mut().find(|e| e.0 == vertex) {
            Some(end) => {
                let keeps = if all { Cost(end.2) == Cost(cost) } else { end.1.len() < k };
                if keeps {
                    end.1.push(entry);
                }
            }
            None => self.ends.push((vertex, vec![entry], cost)),
        }
    }

    /// Whether every end given is complete.
    fn closed(&self, cost: f64) -> bool {
        let all = self.selector == Selector::AllCheapest;
        let done: HashSet<ElementId> =
            self.ends.iter().filter(|e| if all { Cost(e.2) < Cost(cost) } else { e.1.len() >= self.k }).map(|e| e.0).collect();
        self.ctx.all_found(&done)
    }

    /// The walks accepted, each with its cost if the selector is by cost.
    fn bindings(&self) -> Vec<PathBinding> {
        let accepted = self
            .ends
            .iter()
            .flat_map(|(_, entries, _)| entries.iter().map(|&e| (e, self.entries.get(index(e)).map_or(0.0_f64, |x| x.cost))));
        accepted
            .map(|(e, cost)| {
                let mut binding = walk_to(&self.parents, e, &self.states.notes);
                binding.cost = self.ctx.by_cost().then_some(cost);
                binding
            })
            .collect()
    }
}

/// The k cheapest walks from `start` to each end, by `selector`.
///
/// # Errors
///
/// Any error of the view, a prefilter, a cost or a limit.
pub(super) fn ranked(ctx: &Context<'_>, start: ElementId, selector: Selector, meter: &mut Meter) -> Result<Vec<PathBinding>> {
    let k = usize::try_from(selector.k()).unwrap_or(usize::MAX);
    let mut runner = Runner::new(ctx.nfa, ctx.view);
    let mut search = Ranked {
        ctx,
        selector,
        k,
        states: States::default(),
        entries: Vec::new(),
        parents: Vec::new(),
        queue: BinaryHeap::new(),
        taken: HashMap::new(),
        ends: Vec::new(),
    };
    for (note, threads) in runner.start(start)? {
        let letter = search.states.first(start, note);
        search.push((None, letter), threads, 0.0, meter)?;
    }
    'queue: while let Some(Reverse((Cost(cost), entry))) = search.queue.pop() {
        meter.check()?;
        let Some((node, length)) = search.entries.get(index(entry)).map(|e| (e.node, e.length)) else { continue };
        if search.closed(cost) {
            break;
        }
        if !search.take(node, cost) {
            continue;
        }
        if runner.accepts(search.states.threads(node)) {
            search.accept(search.states.vertex(node), entry, cost);
        }
        let successors = search.states.successors(&mut runner, node, length)?;
        if !successors.is_empty() && !meter.length(length.saturating_add(1))? {
            break;
        }
        for successor in successors {
            let step = edge_cost(ctx, successor.hop.edge)?;
            let letter = search.states.letter(&successor);
            if !search.push((Some(entry), letter), successor.threads, cost + step, meter)? {
                break 'queue;
            }
        }
    }
    Ok(search.bindings())
}
