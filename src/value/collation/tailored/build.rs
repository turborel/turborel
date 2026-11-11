//! \[EXTENSION\] A tailoring made from its rules \[UTS #35 Part 5 §3\]: each
//! string a rule names mapped to the elements it then has.
//!
//! A relation puts its string right after the one before it, at its level:
//! greater there, and less than all that came after the one before. So the
//! tailored weights must fit between UNICODE's. Each weight is widened to
//! 64 bits, UNICODE's in the upper 32, and a string put after one of
//! weight `w` takes `w` and a count in the lower 32. Those put after it
//! earlier move up, as `&a<b` then `&a<c` orders `a`, `c`, `b`.
//!
//! While the rules are read, an element names each level by what set it:
//! UNICODE's weight, the common weight of a level below the one changed,
//! or the node a relation made. Each node sits in a run, the nodes after
//! (or, for `[before n]`, before) one level's weight that share the levels
//! above it, linked to the nodes beside it; its weight is its place in
//! that run. Weights are given once every rule is read. A primary of
//! UNICODE's is moved as `[reorder]` says as soon as it is read, so the
//! runs sit among the moved primaries.
//!
//! A reset to `[last regular]` names the reserved primary that begins the
//! group of the unified ideographs, so what follows it is in that group
//! \[UTS #35 Part 5 §3.11\]. One to U+FDD1 and a character names the first
//! primary of that character's group \[UTS #35 Part 5 §2.6.2\]. Under `[strength 3]` the
//! fourth level is not compared, so a relation `<<<<` makes its string
//! equal to the one before.

use std::collections::HashMap;

use super::super::MOST_CONTRACTED;
use super::super::reorder::group_first;
use super::super::rules::{LEVELS, Last, QUATERNARY, Reset, Rule, Rules, damaged};
use super::super::table::Element;
use super::{Base, Built, Prefixes, Strings, Wide, is_upper, matched};
use crate::error::Result;

/// The common weight of the second and third levels \[UTS #10 §6.2\].
const COMMON_SECOND: u16 = 0x0020;
const COMMON_THIRD: u16 = 0x0002;
const COMMON: [u16; LEVELS] = [0, COMMON_SECOND, COMMON_THIRD];
/// How far below a weight the run before it starts.
const BEFORE: u64 = 0x8000_0000;
/// How many bits a weight is widened by.
pub(super) const WIDE_SHIFT: u32 = 32;
/// Where a run after a weight of zero starts: the least weight whose
/// upper bits are not zero, as a key's levels are separated by zero bytes
/// and no UNICODE weight is so low.
const ABOVE_ZERO: u64 = 1 << WIDE_SHIFT;
/// The reserved primary a reset to `[last regular]` names.
const LAST_REGULAR: u16 = 0xFB3F;
/// What, before a character, names the first primary of its group.
const GROUP_FIRST: char = '\u{FDD1}';

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
        Symbol { ids: e.weights.map(Id::Base), variable: e.variable }
    }

    /// The element of first-level weight `primary`, common below it.
    fn primary(primary: u16) -> Symbol {
        Symbol { ids: [Id::Base(primary), Id::Base(COMMON_SECOND), Id::Base(COMMON_THIRD)], variable: false }
    }

    /// The first level at which it is not ignorable, one past the last if
    /// it is ignorable at every level.
    fn level(&self) -> usize {
        self.ids.iter().position(|id| *id != Id::Base(0)).map_or(QUATERNARY, |n| n.saturating_add(1))
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

/// A run and its first and last nodes.
struct Ends {
    run: Run,
    head: Option<usize>,
    tail: Option<usize>,
}

/// A node: its run, and the nodes before and after it there.
struct Node {
    run: usize,
    prev: Option<usize>,
    next: Option<usize>,
}

/// A tailoring being made.
struct Builder<'b> {
    base: &'b Base,
    /// The greatest elements ignorable at the first level, and at the
    /// first two.
    ignorables: [[u16; LEVELS]; 2],
    run_of: HashMap<Run, usize>,
    runs: Vec<Ends>,
    nodes: Vec<Node>,
    map: HashMap<Vec<char>, Vec<Symbol>>,
    prefixed: Prefixes<Symbol>,
    longest: usize,
}

/// The tailoring `rules` make of `base`.
///
/// # Errors
///
/// SQLCODE -916 if a rule cannot be made.
pub(super) fn build(base: Base, rules: &Rules) -> Result<Built> {
    let ignorables = base.unicode.last_ignorables();
    let mut b = Builder {
        base: &base,
        ignorables,
        run_of: HashMap::new(),
        runs: Vec::new(),
        nodes: Vec::new(),
        map: HashMap::new(),
        prefixed: HashMap::new(),
        longest: 0,
    };
    for rule in &rules.list {
        b.rule(rule, rules.strength)?;
    }
    let (map, prefixed, longest) = b.finish()?;
    Ok(Built { map, prefixed, longest, upper_first: rules.upper_first, backwards: rules.backwards, strength: rules.strength, base })
}

impl Builder<'_> {
    /// `rule`'s relations, made under `strength`.
    fn rule(&mut self, rule: &Rule, strength: usize) -> Result<()> {
        let mut reset = self.reset(&rule.reset)?;
        let mut before = rule.before;
        for r in &rule.relations {
            let level = match r.level {
                QUATERNARY if strength < QUATERNARY => 0,
                QUATERNARY => return Err(damaged("a fourth-level relation compared")),
                level => level,
            };
            if level != 0 {
                if before.is_some_and(|b| b != level) {
                    return Err(damaged("a relation after [before n] of another level"));
                }
                // The relation follows the last element that differs at its
                // level or one above; those after it are dropped.
                while reset.len() > 1 && reset.last().is_some_and(|s| s.level() > level) {
                    reset.pop();
                }
                let position = reset.pop().ok_or_else(|| damaged("a reset to nothing"))?;
                reset.push(self.insert(position, level, before.is_some())?);
            }
            before = None;
            let mut elements = reset.clone();
            elements.extend(self.symbols(self.base.unicode.normalized(&r.extension)));
            let text = self.base.unicode.normalized(&r.text);
            if text.len() > MOST_CONTRACTED {
                return Err(damaged("a string of more characters than an element is made of"));
            }
            self.longest = self.longest.max(text.len());
            if r.prefix.is_empty() {
                self.map.insert(text, elements);
            } else {
                let after = self.base.unicode.normalized(&r.prefix);
                let list = self.prefixed.entry(text).or_default();
                list.retain(|(p, _)| *p != after);
                list.push((after, elements));
            }
        }
        Ok(())
    }

    /// The elements a reset names.
    fn reset(&self, reset: &Reset) -> Result<Vec<Symbol>> {
        let weights = |w: [u16; LEVELS]| vec![Symbol { ids: w.map(Id::Base), variable: false }];
        let [primary_ignorable, secondary_ignorable] = self.ignorables;
        let text = match reset {
            Reset::Last(Last::Regular) => return Ok(vec![Symbol::primary(self.base.primary(LAST_REGULAR))]),
            Reset::Last(Last::PrimaryIgnorable) => return Ok(weights(primary_ignorable)),
            Reset::Last(Last::SecondaryIgnorable) => return Ok(weights(secondary_ignorable)),
            Reset::Last(Last::TertiaryIgnorable) => return Ok(weights([0; LEVELS])),
            Reset::Text(text) => self.base.unicode.normalized(text),
        };
        match text.split_first() {
            Some((&GROUP_FIRST, marked)) => {
                let mut elements = Vec::new();
                self.base.step(&mut marked.to_vec(), 0, &mut elements);
                let primary = elements.first().map_or(0, |e| e.weights[0]);
                let first = group_first(primary)?.ok_or_else(|| damaged("a reset to the group of a character in none"))?;
                Ok(vec![Symbol::primary(self.base.primary(first))])
            }
            _ => Ok(self.symbols(text)),
        }
    }

    /// The elements of `chars` as the rules read so far make them.
    fn symbols(&self, mut chars: Vec<char>) -> Vec<Symbol> {
        let mut out = Vec::new();
        let mut at = 0;
        let mut base = Vec::new();
        while at < chars.len() {
            if let Some((found, next)) = matched(&self.map, self.longest, &self.base.unicode, &mut chars, at) {
                out.extend(found);
                at = next;
                continue;
            }
            base.clear();
            at = self.base.step(&mut chars, at, &mut base);
            out.extend(base.iter().map(|e| Symbol::base(self.base.moved(*e))));
        }
        out
    }

    /// A node put after `position` at `level`, or before it: its element.
    fn insert(&mut self, position: Symbol, level: usize, before: bool) -> Result<Symbol> {
        let at = level.checked_sub(1).ok_or_else(|| damaged("a relation of no level"))?;
        let anchor = *position.ids.get(at).ok_or_else(|| damaged("a relation past the third level"))?;
        let node = self.nodes.len();
        match anchor {
            Id::Node(m) => {
                let n = self.nodes.get(m).ok_or_else(|| damaged("a node of no run"))?;
                let (run, prev, next) = (n.run, n.prev, n.next);
                if before { self.link(node, run, prev, Some(m)) } else { self.link(node, run, Some(m), next) }?;
            }
            Id::Base(0) if level == 1 => return Err(damaged("a first-level relation after an ignorable")),
            Id::Base(_) | Id::Common => {
                let key = Run { above: position.ids.get(..at).unwrap_or_default().to_vec(), anchor, before };
                let run = if let Some(run) = self.run_of.get(&key) {
                    *run
                } else {
                    self.run_of.insert(key.clone(), self.runs.len());
                    self.runs.push(Ends { run: key, head: None, tail: None });
                    self.runs.len().saturating_sub(1)
                };
                let ends = self.runs.get(run).ok_or_else(|| damaged("a run of no ends"))?;
                // A later relation goes nearer its reset.
                if before { self.link(node, run, ends.tail, None) } else { self.link(node, run, None, ends.head) }?;
            }
        }
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

    /// Node `node`, new, linked into `run` between `prev` and `next`.
    fn link(&mut self, node: usize, run: usize, prev: Option<usize>, next: Option<usize>) -> Result<()> {
        self.nodes.push(Node { run, prev, next });
        let lost = || damaged("a node linked to none");
        match prev {
            Some(p) => self.nodes.get_mut(p).ok_or_else(lost)?.next = Some(node),
            None => self.runs.get_mut(run).ok_or_else(lost)?.head = Some(node),
        }
        match next {
            Some(n) => self.nodes.get_mut(n).ok_or_else(lost)?.prev = Some(node),
            None => self.runs.get_mut(run).ok_or_else(lost)?.tail = Some(node),
        }
        Ok(())
    }

    /// Each node's weight, from its place in its run.
    fn weights(&self) -> Result<Vec<u64>> {
        let mut out = vec![0; self.nodes.len()];
        for Ends { run, head, .. } in &self.runs {
            let level = run.above.len();
            let anchor = match run.anchor {
                Id::Base(w) => w,
                Id::Common => COMMON.get(level).copied().unwrap_or(0),
                Id::Node(_) => return Err(damaged("a run after a node")),
            };
            let mut weight = u64::from(anchor) << WIDE_SHIFT;
            if weight == 0 && !run.before {
                weight = ABOVE_ZERO;
            }
            if run.before {
                weight = weight.checked_sub(BEFORE).ok_or_else(|| damaged("a weight too low to go before"))?;
            }
            let mut at = *head;
            while let Some(node) = at {
                weight = weight.checked_add(1).ok_or_else(|| damaged("a run too long"))?;
                *out.get_mut(node).ok_or_else(|| damaged("a node of no place"))? = weight;
                at = self.nodes.get(node).and_then(|n| n.next);
            }
        }
        Ok(out)
    }

    /// The strings' elements and the longest string, each symbol given its
    /// weights.
    fn finish(self) -> Result<(Strings, Prefixes<Wide>, usize)> {
        let weights = self.weights()?;
        let base = self.base;
        let wide = |text: &[char], symbols: Vec<Symbol>| -> Result<Vec<Wide>> {
            let upper = base.unicode.elements(text.to_vec()).iter().find(|e| e.weights[2] != 0).is_some_and(|e| is_upper(e.weights[2]));
            symbols.into_iter().map(|s| s.wide(&weights, upper)).collect()
        };
        let mut map = HashMap::with_capacity(self.map.len());
        for (text, symbols) in self.map {
            let elements = wide(&text, symbols)?;
            map.insert(text, elements);
        }
        let mut prefixed = HashMap::with_capacity(self.prefixed.len());
        for (text, list) in self.prefixed {
            let list = list.into_iter().map(|(p, symbols)| Ok((p, wide(&text, symbols)?))).collect::<Result<Vec<_>>>()?;
            prefixed.insert(text, list);
        }
        Ok((map, prefixed, self.longest))
    }
}

impl Symbol {
    /// The element, its nodes given `weights`; of upper case as UNICODE's
    /// weight says, or, if a node is among its levels, as `upper` says.
    fn wide(self, weights: &[u64], upper: bool) -> Result<Wide> {
        let mut wide = [0_u64; LEVELS];
        let mut tailored = false;
        for ((w, id), common) in wide.iter_mut().zip(self.ids).zip(COMMON) {
            *w = match id {
                Id::Base(b) => u64::from(b) << WIDE_SHIFT,
                Id::Common => u64::from(common) << WIDE_SHIFT,
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
