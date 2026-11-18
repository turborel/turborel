//! \[DESIGN\] PathSearch-BiBFS [GS §15]: ANY SHORTEST with both ends
//! bound, by breadth-first searches from the start forwards and from the
//! end backwards over the product of the graph and the automaton, each
//! level taken from the smaller frontier, until they meet.
//!
//! The automaton is run backwards, so its states must be instructions
//! alone: a pattern with no registers, counters or restrictors. A state
//! is a vertex and an instruction waiting to follow an edge, or the end.
//! The walk found is spelled out again forwards, along its elements, to
//! give its letters.

use std::collections::HashMap;

use super::states::Context;
use crate::error::Result;
use crate::graph::binding::{Letter, Notes, PathBinding};
use crate::graph::limits::Meter;
use crate::graph::nfa::Instr;
use crate::graph::run::{Runner, Thread};
use crate::graph::view::{ElementId, Hop};

/// A state: a vertex and an instruction.
type State = (ElementId, usize);

/// One side's search: each state's distance and the step it was reached by.
#[derive(Default)]
struct Side {
    seen: HashMap<State, (usize, Option<(State, ElementId)>)>,
    frontier: Vec<State>,
}

impl Side {
    /// Notes `state` reached by `step` at `distance`, if it is new.
    fn reach(&mut self, state: State, step: Option<(State, ElementId)>, distance: usize, meter: &mut Meter) -> Result<bool> {
        if self.seen.contains_key(&state) {
            return Ok(true);
        }
        self.seen.insert(state, (distance, step));
        self.frontier.push(state);
        meter.state(super::states::STATE_BYTES)
    }
}

/// For each instruction an edge's leads to: the edge instructions it may
/// follow, whatever the vertex's tests.
fn reverse(runner: &Runner<'_>) -> HashMap<usize, Vec<usize>> {
    let nfa = runner.nfa;
    let mut before: HashMap<usize, Vec<usize>> = HashMap::new();
    for (pc, instr) in nfa.instrs.iter().enumerate() {
        if let Instr::Edge { next, .. } = instr {
            for target in reach_ignoring_tests(nfa, *next) {
                before.entry(target).or_default().push(pc);
            }
        }
    }
    before
}

/// The edge instructions and the end reachable from `pc` without an edge,
/// every vertex test passed.
fn reach_ignoring_tests(nfa: &crate::graph::nfa::Nfa, pc: usize) -> Vec<usize> {
    let (mut out, mut seen, mut stack) = (Vec::new(), vec![false; nfa.instrs.len()], vec![pc]);
    while let Some(at) = stack.pop() {
        if seen.get(at).copied().unwrap_or(true) {
            continue;
        }
        if let Some(s) = seen.get_mut(at) {
            *s = true;
        }
        match nfa.instr(at) {
            Some(Instr::Edge { .. } | Instr::Accept) => out.push(at),
            Some(Instr::Split(targets)) => stack.extend(targets.iter().copied()),
            Some(
                Instr::Vertex { next, .. }
                | Instr::Tag { next, .. }
                | Instr::Enter { next, .. }
                | Instr::Leave { next, .. }
                | Instr::Init { next, .. }
                | Instr::Count { next, .. },
            ) => stack.push(*next),
            Some(Instr::Loop { body, exit, .. }) => stack.extend([*body, *exit]),
            None => {}
        }
    }
    out
}

/// The forward level of `side`: each state's steps along its edges.
fn forward(runner: &mut Runner<'_>, side: &mut Side, distance: usize, meter: &mut Meter) -> Result<()> {
    for (vertex, pc) in std::mem::take(&mut side.frontier) {
        meter.check()?;
        let Some(Instr::Edge { orientation, .. }) = runner.nfa.instr(pc) else { continue };
        let mut hops = Vec::new();
        runner.view.hops(vertex, *orientation, &mut hops)?;
        for hop in hops {
            let Some(next) = runner.crosses(pc, &hop)? else { continue };
            for target in runner.arrive(hop.vertex, next)? {
                side.reach((hop.vertex, target), Some(((vertex, pc), hop.edge)), distance, meter)?;
            }
        }
    }
    Ok(())
}

/// The backward level of `side`: the states each state's is reached from.
fn backward(
    runner: &mut Runner<'_>,
    before: &HashMap<usize, Vec<usize>>,
    side: &mut Side,
    distance: usize,
    meter: &mut Meter,
) -> Result<()> {
    for (vertex, pc) in std::mem::take(&mut side.frontier) {
        meter.check()?;
        for &from in before.get(&pc).map_or(&[][..], Vec::as_slice) {
            let Some(Instr::Edge { orientation, .. }) = runner.nfa.instr(from) else { continue };
            let mut hops = Vec::new();
            runner.view.hops(vertex, orientation.reversed(), &mut hops)?;
            for back in hops {
                // The hop followed forwards, from its other end.
                let hop = Hop { edge: back.edge, vertex, way: reversed(back.way) };
                let Some(next) = runner.crosses(from, &hop)? else { continue };
                if runner.arrive(vertex, next)?.contains(&pc) {
                    side.reach((back.vertex, from), Some(((vertex, pc), back.edge)), distance, meter)?;
                }
            }
        }
    }
    Ok(())
}

/// The way an edge followed `way` is followed from its other end.
fn reversed(way: crate::graph::view::Way) -> crate::graph::view::Way {
    use crate::graph::view::Way;
    match way {
        Way::Right => Way::Left,
        Way::Left => Way::Right,
        Way::Undirected => Way::Undirected,
    }
}

/// ANY SHORTEST from `start` to `end`.
///
/// # Errors
///
/// Any error of the view, a prefilter or a limit.
pub(super) fn bidirectional(ctx: &Context<'_>, (start, end): (ElementId, ElementId), meter: &mut Meter) -> Result<Vec<PathBinding>> {
    let mut runner = Runner::new(ctx.nfa, ctx.view);
    let before = reverse(&runner);
    let (mut ahead, mut behind) = (Side::default(), Side::default());
    for pc in runner.arrive(start, ctx.nfa.start)? {
        ahead.reach((start, pc), None, 0, meter)?;
    }
    let accept = ctx.nfa.instrs.iter().position(|i| matches!(i, Instr::Accept)).unwrap_or(0);
    behind.reach((end, accept), None, 0, meter)?;
    let (mut forwards, mut backwards) = (0_usize, 0_usize);
    loop {
        let meet =
            ahead.seen.iter().filter_map(|(state, (d, _))| behind.seen.get(state).map(|(b, _)| (d.saturating_add(*b), *state))).min();
        if let Some((_, state)) = meet {
            return spell(&mut runner, (&ahead, &behind), state);
        }
        if ahead.frontier.is_empty() || behind.frontier.is_empty() || !meter.length(forwards.saturating_add(backwards).saturating_add(1))? {
            return Ok(Vec::new());
        }
        if ahead.frontier.len() <= behind.frontier.len() {
            forwards = forwards.saturating_add(1);
            forward(&mut runner, &mut ahead, forwards, meter)?;
        } else {
            backwards = backwards.saturating_add(1);
            backward(&mut runner, &before, &mut behind, backwards, meter)?;
        }
    }
}

/// The binding of the walk through `state`, its elements read back from
/// both sides and its letters spelled forwards.
fn spell(runner: &mut Runner<'_>, (ahead, behind): (&Side, &Side), state: State) -> Result<Vec<PathBinding>> {
    let mut steps: Vec<(ElementId, ElementId)> = Vec::new();
    let mut at = state;
    while let Some((_, Some((from, edge)))) = ahead.seen.get(&at) {
        steps.push((*edge, at.0));
        at = *from;
    }
    let start = at.0;
    steps.reverse();
    at = state;
    while let Some((_, Some((to, edge)))) = behind.seen.get(&at) {
        steps.push((*edge, to.0));
        at = *to;
    }
    let mut notes = Notes::default();
    let mut letters = Vec::new();
    for (note, threads) in runner.start(start)? {
        letters.push(Letter { edge: None, vertex: start, note: notes.id(note) });
        if along(runner, &mut notes, (&threads, &steps), &mut letters)? {
            return Ok(vec![PathBinding::of(&letters, &notes)]);
        }
        letters.clear();
    }
    Ok(Vec::new())
}

/// Spells the letters of `steps` from `threads` into `letters`, trying
/// each way of binding them, depth first: whether one ends accepted.
fn along(
    runner: &mut Runner<'_>,
    notes: &mut Notes,
    (threads, steps): (&[Thread], &[(ElementId, ElementId)]),
    letters: &mut Vec<Letter>,
) -> Result<bool> {
    let mut stack: Vec<(Vec<crate::graph::run::Successor>, usize)> = Vec::new();
    let mut current: Vec<Thread> = threads.to_vec();
    loop {
        let depth = stack.len();
        if let Some(&(edge, vertex)) = steps.get(depth) {
            let from = letters.last().map_or(vertex, |l| l.vertex);
            let mut successors = Vec::new();
            runner.successors(from, (depth, None), &current, &mut successors)?;
            successors.retain(|s| (s.hop.edge, s.hop.vertex) == (edge, vertex));
            stack.push((successors, 0));
        } else if runner.accepts(&current) {
            return Ok(true);
        }
        loop {
            let Some((successors, next)) = stack.last_mut() else { return Ok(false) };
            if let Some(s) = successors.get(*next) {
                *next = next.saturating_add(1);
                let letter = Letter { edge: Some((s.hop.edge, s.var)), vertex: s.hop.vertex, note: notes.id(s.note.clone()) };
                current.clone_from(&s.threads);
                let keep = stack.len();
                letters.truncate(keep);
                letters.push(letter);
                break;
            }
            stack.pop();
        }
    }
}
