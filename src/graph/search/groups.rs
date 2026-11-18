//! \[DESIGN\] PathSearch-AllShort [GS §15]: ALL SHORTEST and SHORTEST k
//! GROUP by a breadth-first search keeping, for each state reached at
//! each length, every state and letter it is reached from: a predecessor
//! DAG whose walks to an end are its walks of that length.
//!
//! A state is kept at its k shortest lengths alone. A walk to an end at
//! one of the end's k shortest lengths has its every prefix at one of
//! its state's k shortest lengths, or the end would have k shorter ones,
//! the same suffix after each; so the DAG holds every walk the selector
//! keeps, and the search, at most k levels of each state, ends.

use std::collections::{HashMap, HashSet};

use super::states::{Context, States, index};
use crate::error::Result;
use crate::graph::binding::{Letter, PathBinding};
use crate::graph::limits::Meter;
use crate::graph::run::Runner;
use crate::graph::view::ElementId;

/// A state at one length, and what it is reached from.
struct Record {
    preds: Vec<(Option<u32>, Letter)>,
}

/// An end's lengths found and the records accepting there.
#[derive(Default)]
struct End {
    lengths: Vec<usize>,
    records: Vec<u32>,
}

/// The search's records and ends.
struct Dag<'c, 's> {
    ctx: &'c Context<'s>,
    k: usize,
    states: States,
    records: Vec<Record>,
    /// The lengths each state is kept at so far.
    levels: HashMap<u32, usize>,
    ends: Vec<(ElementId, End)>,
}

impl Dag<'_, '_> {
    /// Notes the record `record`, of a state accepting at `vertex`, at `length`.
    fn accept(&mut self, vertex: ElementId, length: usize, record: u32) {
        if !self.ctx.ends_at(vertex) {
            return;
        }
        let at = self.ends.iter().position(|e| e.0 == vertex).unwrap_or_else(|| {
            self.ends.push((vertex, End::default()));
            self.ends.len().saturating_sub(1)
        });
        let Some((_, end)) = self.ends.get_mut(at) else { return };
        let fits = end.lengths.last() == Some(&length) || end.lengths.len() < self.k;
        if fits {
            if end.lengths.last() != Some(&length) {
                end.lengths.push(length);
            }
            end.records.push(record);
        }
    }

    /// Whether every end given has its k lengths, all shorter than `length`.
    fn closed(&self, length: usize) -> bool {
        let done: HashSet<ElementId> = self
            .ends
            .iter()
            .filter(|(_, e)| e.lengths.len() >= self.k && e.lengths.last().is_some_and(|&l| l < length))
            .map(|e| e.0)
            .collect();
        self.ctx.all_found(&done)
    }

    /// The record of `vertex` and `threads` at `length`, reached from
    /// `pred` by `letter`: added to this level's, or made if its state has
    /// fewer than k lengths. Whether the search goes on.
    fn reach(
        &mut self,
        level: &mut HashMap<u32, u32>,
        (vertex, threads): (ElementId, Vec<crate::graph::run::Thread>),
        (pred, letter): (Option<u32>, Letter),
        (length, meter): (usize, &mut Meter),
    ) -> Result<bool> {
        let accepting = threads.iter().any(|t| matches!(self.ctx.nfa.instr(t.pc), Some(crate::graph::nfa::Instr::Accept)));
        let (node, _) = self.states.node(vertex, threads);
        if let Some(record) = level.get(&node).and_then(|&r| self.records.get_mut(index(r))) {
            record.preds.push((pred, letter));
            return Ok(true);
        }
        let kept = self.levels.entry(node).or_insert(0);
        if *kept >= self.k {
            return Ok(true);
        }
        *kept = kept.saturating_add(1);
        let record = u32::try_from(self.records.len()).unwrap_or(u32::MAX);
        self.records.push(Record { preds: vec![(pred, letter)] });
        level.insert(node, record);
        if accepting {
            self.accept(vertex, length, record);
        }
        meter.state(self.states.bytes(node))
    }

    /// The walks to record `record`, each a list of letters, appended to `out`.
    fn walks(&self, record: u32, meter: &mut Meter, out: &mut Vec<Vec<Letter>>) -> Result<bool> {
        let mut stack: Vec<(u32, Vec<Letter>)> = vec![(record, Vec::new())];
        while let Some((at, suffix)) = stack.pop() {
            meter.check()?;
            let Some(r) = self.records.get(index(at)) else { continue };
            for (pred, letter) in r.preds.iter().rev() {
                let mut walk = suffix.clone();
                walk.push(*letter);
                if let Some(p) = pred {
                    stack.push((*p, walk));
                    continue;
                }
                walk.reverse();
                out.push(walk);
                if !meter.paths(out.len())? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

/// ALL SHORTEST (k = 1) and SHORTEST k GROUP from `start`.
///
/// # Errors
///
/// Any error of the view, a prefilter or a limit.
pub(super) fn groups(ctx: &Context<'_>, start: ElementId, k: u64, meter: &mut Meter) -> Result<Vec<PathBinding>> {
    let k = usize::try_from(k).unwrap_or(usize::MAX);
    let mut runner = Runner::new(ctx.nfa, ctx.view);
    let mut dag = Dag { ctx, k, states: States::default(), records: Vec::new(), levels: HashMap::new(), ends: Vec::new() };
    let mut level = HashMap::new();
    for (note, threads) in runner.start(start)? {
        let letter = dag.states.first(start, note);
        dag.reach(&mut level, (start, threads), (None, letter), (0, meter))?;
    }
    let mut length = 0_usize;
    'levels: while !level.is_empty() && !dag.closed(length) {
        let mut frontier: Vec<(u32, u32)> = level.drain().collect();
        frontier.sort_unstable_by_key(|&(_, record)| record);
        for (node, record) in frontier {
            meter.check()?;
            let successors = dag.states.successors(&mut runner, node, length)?;
            if !successors.is_empty() && !meter.length(length.saturating_add(1))? {
                break 'levels;
            }
            for successor in successors {
                let letter = dag.states.letter(&successor);
                let reached = (successor.hop.vertex, successor.threads);
                if !dag.reach(&mut level, reached, (Some(record), letter), (length.saturating_add(1), meter))? {
                    break 'levels;
                }
            }
        }
        length = length.saturating_add(1);
    }
    let mut bindings = Vec::new();
    for (_, end) in &dag.ends {
        let mut walks = Vec::new();
        for &record in &end.records {
            if !dag.walks(record, meter, &mut walks)? {
                break;
            }
        }
        bindings.extend(walks.iter().map(|w| PathBinding::of(w, &dag.states.notes)));
    }
    Ok(bindings)
}
