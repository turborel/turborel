//! \[DESIGN\] MS-BFS [GS §15, §17]: ANY SHORTEST from up to 64 starts at
//! once, one breadth-first search whose frontier and seen sets are 64-lane
//! bitsets over its states, after Then et al.'s multi-source BFS. A state
//! reached at one level from several starts is expanded once for all of
//! them.
//!
//! Each lane keeps, of the parents it reaches a state from at a level,
//! the one a search of its own would: the earliest of its level, the
//! earliest of its steps. So each start's walks are the ones `bfs.rs`
//! finds, however the lanes share the work.

use std::collections::{HashMap, HashSet};

use super::states::{Context, States, index, walk_to};
use crate::error::Result;
use crate::graph::binding::{Letter, PathBinding};
use crate::graph::limits::Meter;
use crate::graph::run::{Runner, Thread};
use crate::graph::view::ElementId;

/// The starts a batch searches at once: the lanes of a word.
pub const LANES: usize = 64;

/// The place a lane reaches a state from at a level: its parent's rank
/// in the lane's level, and the step's among the parent's.
type Key = (usize, usize);

/// A state a lane reaches at a level: its key, the state, its parent and letter.
type Reached = (Key, u32, Option<u32>, Letter);

/// One batch's states, lanes, parents and ranks.
struct Batch {
    states: States,
    /// The lanes that have seen each state.
    seen: Vec<u64>,
    /// Whether each state accepts.
    accepting: Vec<bool>,
    /// Each lane's parent of each state it saw, and the letter.
    parents: HashMap<(u32, usize), (Option<u32>, Letter)>,
    /// Each lane's rank of each state of its last level.
    ranks: HashMap<(u32, usize), usize>,
    /// Each lane's ends found, and the state accepting there.
    found: Vec<Vec<(ElementId, u32)>>,
    ends: Vec<HashSet<ElementId>>,
}

impl Batch {
    /// The state of `vertex` and `threads`, counted if new.
    fn node(&mut self, ctx: &Context<'_>, vertex: ElementId, threads: Vec<Thread>, meter: &mut Meter) -> Result<(u32, bool)> {
        let accepting = threads.iter().any(|t| matches!(ctx.nfa.instr(t.pc), Some(crate::graph::nfa::Instr::Accept)));
        let (node, fresh) = self.states.node(vertex, threads);
        if !fresh {
            return Ok((node, true));
        }
        self.seen.push(0);
        self.accepting.push(accepting);
        Ok((node, meter.state(self.states.bytes(node))?))
    }

    /// Settles a level: each lane's new states, by their keys, get their
    /// parents, ranks and ends. Gives the next frontier.
    fn settle(&mut self, ctx: &Context<'_>, candidates: HashMap<(u32, usize), (Key, Option<u32>, Letter)>) -> HashMap<u32, u64> {
        let mut by_lane: Vec<Vec<Reached>> = vec![Vec::new(); self.found.len()];
        for ((node, lane), (key, parent, letter)) in candidates {
            if let Some(list) = by_lane.get_mut(lane) {
                list.push((key, node, parent, letter));
            }
        }
        self.ranks.clear();
        let mut frontier: HashMap<u32, u64> = HashMap::new();
        for (lane, mut list) in by_lane.into_iter().enumerate() {
            list.sort_unstable_by_key(|entry| entry.0);
            for (rank, (_, node, parent, letter)) in list.into_iter().enumerate() {
                self.parents.insert((node, lane), (parent, letter));
                self.ranks.insert((node, lane), rank);
                if let Some(seen) = self.seen.get_mut(index(node)) {
                    *seen |= lane_bit(lane);
                }
                *frontier.entry(node).or_insert(0) |= lane_bit(lane);
                let accepts = self.accepting.get(index(node)).copied().unwrap_or(false) && ctx.ends_at(letter.vertex);
                let found =
                    self.found.get_mut(lane).filter(|_| accepts && self.ends.get_mut(lane).is_some_and(|e| e.insert(letter.vertex)));
                if let Some(f) = found {
                    f.push((letter.vertex, node));
                }
            }
        }
        frontier
    }

    /// The walk of lane `lane` to state `node`.
    fn walk(&self, lane: usize, node: u32) -> PathBinding {
        let mut letters = Vec::new();
        let mut at = Some(node);
        while let Some((parent, letter)) = at.and_then(|n| self.parents.get(&(n, lane))) {
            letters.push(*letter);
            at = *parent;
        }
        letters.reverse();
        let parents: Vec<(Option<u32>, Letter)> =
            letters.iter().enumerate().map(|(i, l)| (i.checked_sub(1).and_then(|p| u32::try_from(p).ok()), *l)).collect();
        let last = u32::try_from(parents.len().saturating_sub(1)).unwrap_or(0);
        walk_to(&parents, last, &self.states.notes)
    }
}

/// The bit of lane `lane`.
fn lane_bit(lane: usize) -> u64 {
    u32::try_from(lane).ok().and_then(|shift| 1_u64.checked_shl(shift)).unwrap_or(0)
}

/// The lanes set in `mask`.
fn lanes(mask: u64) -> impl Iterator<Item = usize> {
    (0..LANES).filter(move |&lane| mask & lane_bit(lane) != 0)
}

/// Keeps, for each lane of `mask` new to `node`, the least-keyed way it is reached.
fn offer(
    candidates: &mut HashMap<(u32, usize), (Key, Option<u32>, Letter)>,
    (node, mask): (u32, u64),
    ranks: &HashMap<(u32, usize), usize>,
    (parent, step, letter): (Option<u32>, usize, Letter),
) {
    for lane in lanes(mask) {
        let rank = parent.and_then(|p| ranks.get(&(p, lane)).copied()).unwrap_or(0);
        let key = (rank, step);
        let entry = candidates.entry((node, lane)).or_insert((key, parent, letter));
        if key < entry.0 {
            *entry = (key, parent, letter);
        }
    }
}

/// ANY SHORTEST from each of `starts`, at most `LANES` of them: the
/// walks of each start, in the order of the starts.
///
/// # Errors
///
/// Any error of the view, a prefilter or a limit.
pub(super) fn multi_source(ctx: &Context<'_>, starts: &[ElementId], meter: &mut Meter) -> Result<Vec<Vec<PathBinding>>> {
    let mut runner = Runner::new(ctx.nfa, ctx.view);
    let count = starts.len().min(LANES);
    let mut batch = Batch {
        states: States::default(),
        seen: Vec::new(),
        accepting: Vec::new(),
        parents: HashMap::new(),
        ranks: HashMap::new(),
        found: vec![Vec::new(); count],
        ends: vec![HashSet::new(); count],
    };
    let mut candidates = HashMap::new();
    for (lane, &start) in starts.iter().take(LANES).enumerate() {
        for (step, (note, threads)) in runner.start(start)?.into_iter().enumerate() {
            let letter = batch.states.first(start, note);
            let (node, _) = batch.node(ctx, start, threads, meter)?;
            offer(&mut candidates, (node, lane_bit(lane)), &batch.ranks, (None, step, letter));
        }
    }
    let mut frontier = batch.settle(ctx, candidates);
    let mut level = 0_usize;
    'levels: while !frontier.is_empty() {
        let mut ordered: Vec<(u32, u64)> = frontier.drain().collect();
        ordered.sort_unstable_by_key(|&(node, _)| node);
        let mut offers = HashMap::new();
        for (node, mask) in ordered {
            meter.check()?;
            let successors = batch.states.successors(&mut runner, node, level)?;
            if !successors.is_empty() && !meter.length(level.saturating_add(1))? {
                break 'levels;
            }
            for (step, successor) in successors.into_iter().enumerate() {
                let letter = batch.states.letter(&successor);
                let (reached, go) = batch.node(ctx, successor.hop.vertex, successor.threads, meter)?;
                let new = mask & !batch.seen.get(index(reached)).copied().unwrap_or(0);
                offer(&mut offers, (reached, new), &batch.ranks, (Some(node), step, letter));
                if !go {
                    break 'levels;
                }
            }
        }
        frontier = batch.settle(ctx, offers);
        level = level.saturating_add(1);
    }
    Ok((0..count)
        .map(|lane| batch.found.get(lane).map_or_else(Vec::new, |f| f.iter().map(|&(_, node)| batch.walk(lane, node)).collect()))
        .collect())
}
