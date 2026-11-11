//! \[EXTENSION\] A tailoring made from its rules \[UTS #35 Part 5 §3\]: each
//! string a rule names mapped to the elements it then has.
//!
//! A relation puts its string right after the one before it, at its level:
//! greater there, and less than all that came after the one before. So the
//! tailored weights must fit between UNICODE's. Each weight is widened to
//! 32 bits, UNICODE's in the upper 16, and a string put after one of
//! weight `w` takes `w` and a count in the lower 16. Those put after it
//! earlier move up, as `&a<b` then `&a<c` orders `a`, `c`, `b`.
//!
//! While the rules are read, an element names each level by what set it:
//! UNICODE's weight, the common weight of a level below the one changed,
//! or the node a relation made. Each node sits in a run, the nodes after
//! (or, for `[before n]`, before) one level's weight that share the levels
//! above it; its weight is its place in that run. Weights are given once
//! every rule is read.

use std::collections::HashMap;

use super::super::Unicode;
use super::super::rules::{LEVELS, Rules, damaged};
use super::super::table::Element;
use super::{Built, Wide, is_upper, matched};
use crate::error::Result;

/// The common weight of the second and third levels \[UTS #10 §6.2\].
const COMMON: [u16; LEVELS] = [0, 0x0020, 0x0002];
/// How far below a weight the run before it starts.
const BEFORE: u32 = 0x8000;
/// How many bits a weight is widened by.
pub(super) const WIDE_SHIFT: u32 = 16;

/// What set one level's weight of an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Id {
    /// UNICODE's weight.
    Base(u16),
    /// The common weight, of a level below the one a relation changed.
    Common,
    /// A relation's node.
    Node(usize),
}

/// An element while the rules are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Symbol {
    ids: [Id; LEVELS],
    variable: bool,
}

impl Symbol {
    fn base(e: Element) -> Symbol {
        let [p, s, t] = e.weights;
        Symbol { ids: [Id::Base(p), Id::Base(s), Id::Base(t)], variable: e.variable }
    }
}

/// Where a run of nodes sits: the levels above its own, the weight it
/// follows or precedes, and which.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Run {
    above: Vec<Id>,
    anchor: Id,
    before: bool,
}

/// A tailoring being made.
struct Builder<'u> {
    unicode: &'u Unicode,
    runs: HashMap<Run, Vec<usize>>,
    /// Each node's run.
    nodes: Vec<Run>,
    map: HashMap<Vec<char>, Vec<Symbol>>,
    longest: usize,
}

/// The tailoring `rules` make of `unicode`.
///
/// # Errors
///
/// SQLCODE -916 if a rule cannot be made.
pub(super) fn build(unicode: &Unicode, rules: &Rules) -> Result<Built> {
    let mut b = Builder { unicode, runs: HashMap::new(), nodes: Vec::new(), map: HashMap::new(), longest: 0 };
    for rule in &rules.list {
        let mut prefix = b.symbols(unicode.normalized(&rule.reset));
        let mut position = prefix.pop().ok_or_else(|| damaged("a reset to nothing"))?;
        let mut before = rule.before;
        for r in &rule.relations {
            if r.level != 0 {
                if before.is_some_and(|level| level != r.level) {
                    return Err(damaged("a relation after [before n] of another level"));
                }
                position = b.insert(position, r.level, before.is_some())?;
            }
            before = None;
            let mut elements = prefix.clone();
            elements.push(position);
            elements.extend(b.symbols(unicode.normalized(&r.extension)));
            let text = unicode.normalized(&r.text);
            b.longest = b.longest.max(text.len());
            b.map.insert(text, elements);
        }
        prefix.clear();
    }
    b.finish(rules)
}

impl Builder<'_> {
    /// The elements of `chars` as the rules read so far make them.
    fn symbols(&self, mut chars: Vec<char>) -> Vec<Symbol> {
        let mut out = Vec::new();
        let mut at = 0;
        let mut base = Vec::new();
        while at < chars.len() {
            if let Some((found, next)) = matched(&self.map, self.longest, self.unicode, &mut chars, at) {
                out.extend(found);
                at = next;
                continue;
            }
            base.clear();
            at = self.unicode.step(&mut chars, at, &mut base);
            out.extend(base.iter().copied().map(Symbol::base));
        }
        out
    }

    /// A node put after `position` at `level`, or before it: its element.
    fn insert(&mut self, position: Symbol, level: usize, before: bool) -> Result<Symbol> {
        let at = level.checked_sub(1).ok_or_else(|| damaged("a relation of no level"))?;
        let anchor = *position.ids.get(at).ok_or_else(|| damaged("a relation past the third level"))?;
        let node = self.nodes.len();
        let run = match anchor {
            Id::Node(m) => {
                let run = self.nodes.get(m).cloned().ok_or_else(|| damaged("a node of no run"))?;
                let nodes = self.runs.entry(run.clone()).or_default();
                let place = nodes.iter().position(|n| *n == m).unwrap_or(0);
                nodes.insert(if before { place } else { place.saturating_add(1) }, node);
                run
            }
            Id::Base(0) if level == 1 => return Err(damaged("a first-level relation after an ignorable")),
            Id::Base(_) | Id::Common => {
                let run = Run { above: position.ids.get(..at).unwrap_or_default().to_vec(), anchor, before };
                let nodes = self.runs.entry(run.clone()).or_default();
                if before {
                    nodes.push(node);
                } else {
                    nodes.insert(0, node);
                }
                run
            }
        };
        self.nodes.push(run);
        let mut ids = [Id::Common; LEVELS];
        for (n, id) in ids.iter_mut().enumerate() {
            *id = match n.cmp(&at) {
                std::cmp::Ordering::Less => position.ids.get(n).copied().unwrap_or(Id::Common),
                std::cmp::Ordering::Equal => Id::Node(node),
                std::cmp::Ordering::Greater => Id::Common,
            };
        }
        Ok(Symbol { ids, variable: position.variable })
    }

    /// Each node's weight, from its place in its run.
    fn weights(&self) -> Result<Vec<u32>> {
        let mut out = vec![0; self.nodes.len()];
        for (run, nodes) in &self.runs {
            let level = run.above.len();
            let anchor = match run.anchor {
                Id::Base(w) => w,
                Id::Common => COMMON.get(level).copied().unwrap_or(0),
                Id::Node(_) => return Err(damaged("a run after a node")),
            };
            let mut weight = u32::from(anchor) << WIDE_SHIFT;
            if run.before {
                weight = weight.checked_sub(BEFORE).ok_or_else(|| damaged("a weight too low to go before"))?;
            }
            for node in nodes {
                weight = weight.checked_add(1).ok_or_else(|| damaged("a run too long"))?;
                *out.get_mut(*node).ok_or_else(|| damaged("a node of no place"))? = weight;
            }
        }
        Ok(out)
    }

    /// The tailoring, each symbol given its weights.
    fn finish(self, rules: &Rules) -> Result<Built> {
        let weights = self.weights()?;
        let mut map = HashMap::with_capacity(self.map.len());
        for (text, symbols) in self.map {
            let upper = self.unicode.elements(text.clone()).iter().find(|e| e.weights[2] != 0).is_some_and(|e| is_upper(e.weights[2]));
            let elements = symbols.into_iter().map(|s| s.wide(&weights, upper)).collect::<Result<Vec<Wide>>>()?;
            map.insert(text, elements);
        }
        Ok(Built { map, longest: self.longest, upper_first: rules.upper_first, backwards: rules.backwards })
    }
}

impl Symbol {
    /// The element, its nodes given `weights`; of upper case as UNICODE's
    /// weight says, or, if a node is among its levels, as `upper` says.
    fn wide(self, weights: &[u32], upper: bool) -> Result<Wide> {
        let mut wide = [0_u32; LEVELS];
        let mut tailored = false;
        for ((w, id), common) in wide.iter_mut().zip(self.ids).zip(COMMON) {
            *w = match id {
                Id::Base(b) => u32::from(b) << WIDE_SHIFT,
                Id::Common => u32::from(common) << WIDE_SHIFT,
                Id::Node(n) => {
                    tailored = true;
                    weights.get(n).copied().ok_or_else(|| damaged("a node of no weight"))?
                }
            };
        }
        let base_upper = matches!(self.ids.get(2), Some(Id::Base(t)) if is_upper(*t));
        Ok(Wide { weights: wide, variable: self.variable, upper: if tailored { upper } else { base_upper } })
    }
}
