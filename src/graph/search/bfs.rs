//! \[DESIGN\] PathSearch-BFS [GS §15]: breadth-first search of the product
//! of the graph and the automaton from one start, for ANY SHORTEST and
//! ANY. The first state accepting at a vertex is reached by a shortest
//! walk there, kept by its parent pointers.

use std::collections::HashSet;

use super::states::{Context, States, walk_to};
use crate::error::Result;
use crate::graph::binding::{Letter, PathBinding};
use crate::graph::limits::Meter;
use crate::graph::run::Runner;
use crate::graph::view::ElementId;

/// A breadth-first search's states, each with its parent and letter, and
/// the first state accepting at each end found.
#[derive(Default)]
pub(super) struct Tree {
    pub(super) states: States,
    pub(super) parents: Vec<(Option<u32>, Letter)>,
    pub(super) found: Vec<(ElementId, u32)>,
    pub(super) ends: HashSet<ElementId>,
}

impl Tree {
    /// The tree of a search of `ctx`.
    pub(super) fn of(ctx: &Context<'_>) -> Tree {
        Tree { states: States::of(ctx.nfa), ..Tree::default() }
    }

    /// Adds the state of `vertex` and `threads`, reached from `parent` by
    /// `letter`, if it is new: whether the search goes on, and the state
    /// if it is new.
    pub(super) fn add(
        &mut self,
        ctx: &Context<'_>,
        (parent, letter): (Option<u32>, Letter),
        threads: Vec<crate::graph::run::Thread>,
        meter: &mut Meter,
    ) -> Result<(bool, Option<u32>)> {
        let accepting = ctx_accepts(ctx, &threads);
        let (node, fresh) = self.states.node(letter.vertex, threads);
        if !fresh {
            return Ok((true, None));
        }
        self.parents.push((parent, letter));
        if accepting && ctx.ends_at(letter.vertex) && self.ends.insert(letter.vertex) {
            self.found.push((letter.vertex, node));
        }
        Ok((meter.state(self.states.bytes(node))?, Some(node)))
    }

    /// The states a search from `start` begins in.
    pub(super) fn begin(&mut self, ctx: &Context<'_>, runner: &mut Runner<'_>, start: ElementId, meter: &mut Meter) -> Result<Vec<u32>> {
        let mut frontier = Vec::new();
        for (note, threads) in runner.start(start)? {
            let letter = self.states.first(start, note);
            let (go, node) = self.add(ctx, (None, letter), threads, meter)?;
            frontier.extend(node);
            if !go {
                break;
            }
        }
        Ok(frontier)
    }

    /// The bindings of the walks found, one an end.
    pub(super) fn bindings(&self) -> Vec<PathBinding> {
        self.found.iter().map(|&(_, node)| walk_to(&self.parents, node, &self.states.notes)).collect()
    }
}

/// Whether `threads` include one that matched.
fn ctx_accepts(ctx: &Context<'_>, threads: &[crate::graph::run::Thread]) -> bool {
    threads.iter().any(|t| matches!(ctx.nfa.instr(t.pc), Some(crate::graph::nfa::Instr::Accept)))
}

/// ANY SHORTEST from `start`: a shortest walk to each end it reaches.
///
/// # Errors
///
/// Any error of the view, a prefilter or a limit.
pub(super) fn any_shortest(ctx: &Context<'_>, start: ElementId, meter: &mut Meter) -> Result<Vec<PathBinding>> {
    let mut runner = Runner::new(ctx.nfa, ctx.view);
    let mut tree = Tree::of(ctx);
    let mut frontier = tree.begin(ctx, &mut runner, start, meter)?;
    let mut level = 0_usize;
    'levels: while !frontier.is_empty() && !ctx.all_found(&tree.ends) {
        let mut next = Vec::new();
        for node in frontier {
            meter.check()?;
            let successors = tree.states.successors(&mut runner, node, level)?;
            if !successors.is_empty() && !meter.length(level.saturating_add(1))? {
                break 'levels;
            }
            for successor in successors {
                let letter = tree.states.letter(&successor);
                let (go, added) = tree.add(ctx, (Some(node), letter), successor.threads, meter)?;
                next.extend(added);
                if !go {
                    break 'levels;
                }
            }
        }
        frontier = next;
        level = level.saturating_add(1);
    }
    Ok(tree.bindings())
}

/// \[DESIGN\] The states of a level each worker of a parallel search
/// expands at a time.
pub const LEVEL_MORSEL: usize = 64;

/// ANY SHORTEST from `start`, level-synchronous on `width` workers [GS
/// §17]: each level's states expanded in parallel, their successors then
/// added in the level's order, as `any_shortest` adds them.
///
/// # Errors
///
/// As `any_shortest`; -910 if a worker fails.
pub(super) fn any_shortest_levels(ctx: &Context<'_>, start: ElementId, width: usize, meter: &mut Meter) -> Result<Vec<PathBinding>> {
    let mut runner = Runner::new(ctx.nfa, ctx.view);
    let mut tree = Tree::of(ctx);
    let mut frontier = tree.begin(ctx, &mut runner, start, meter)?;
    let mut level = 0_usize;
    'levels: while !frontier.is_empty() && !ctx.all_found(&tree.ends) {
        let states = &tree.states;
        let expanded = crate::threads::morsels(width, (frontier.len(), LEVEL_MORSEL), meter.stop(), |range| {
            let mut worker = Runner::new(ctx.nfa, ctx.view);
            frontier
                .get(range)
                .unwrap_or_default()
                .iter()
                .map(|&node| states.successors(&mut worker, node, level))
                .collect::<Result<Vec<_>>>()
        })?;
        let mut next = Vec::new();
        for (node, successors) in frontier.iter().zip(expanded.into_iter().flatten()) {
            if !successors.is_empty() && !meter.length(level.saturating_add(1))? {
                break 'levels;
            }
            for successor in successors {
                let letter = tree.states.letter(&successor);
                let (go, added) = tree.add(ctx, (Some(*node), letter), successor.threads, meter)?;
                next.extend(added);
                if !go {
                    break 'levels;
                }
            }
        }
        frontier = next;
        level = level.saturating_add(1);
    }
    Ok(tree.bindings())
}
