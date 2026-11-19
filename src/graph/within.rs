//! \[EXTENSION\] EXISTS IN PATH [GS §11.2]: whether a pattern matches the
//! subgraph of a path's elements, in path order. The pattern's automaton
//! is run along the path from each of its vertices, each step the next
//! edge of the path to its next vertex alone, until a thread accepts;
//! the path is finite, so no selector or restrictor is needed for it to
//! end (RULES.md D142).

use super::nfa::Nfa;
use super::program::{PathPattern, Selector};
use super::run::{Runner, Thread, Walk};
use super::view::{ElementId, GraphView, Orientation};
use crate::error::Result;

/// Whether `pattern` matches a part of the path of `elements`, a vertex
/// first and last and edges between, walked in its order.
///
/// # Errors
///
/// Any error of the view or a prefilter of the pattern.
pub fn within(pattern: &PathPattern, view: &dyn GraphView, elements: &[ElementId]) -> Result<bool> {
    // Only whether one match exists is asked, which ANY selects.
    let nfa = Nfa::compile(&pattern.clone().select(Selector::Any(1)))?;
    let mut runner = Runner::new(&nfa, view);
    for start in (0..elements.len()).step_by(2) {
        let Some(&first) = elements.get(start) else { continue };
        let mut threads: Vec<Thread> = runner.start(first)?.into_iter().flat_map(|(_, t)| t).collect();
        let mut walk = Walk::default();
        walk.begin(first);
        let mut at = start;
        while !threads.is_empty() {
            if runner.accepts(&threads) {
                return Ok(true);
            }
            let (Some(&vertex), Some(&edge), Some(&next)) =
                (elements.get(at), elements.get(at.saturating_add(1)), elements.get(at.saturating_add(2)))
            else {
                break;
            };
            let mut hops = Vec::new();
            view.hops(vertex, Orientation::ANY, &mut hops)?;
            hops.retain(|h| h.edge == edge && h.vertex == next);
            let mut out = Vec::new();
            let position = at.saturating_sub(start).div_euclid(2);
            runner.along(&hops, (position, Some(&walk)), &threads, &mut out)?;
            threads = out.into_iter().flat_map(|s| s.threads).collect();
            threads.sort_unstable();
            threads.dedup();
            walk.push(edge, next);
            at = at.saturating_add(2);
        }
    }
    Ok(false)
}
