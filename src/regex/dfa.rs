//! \[DESIGN\] (RULES.md D1188) Where a program's match ends, found by an
//! automaton made as the text is read (Thompson, "Regular Expression
//! Search Algorithm", CACM 1968; Cox, "Regular Expression Matching in the
//! Wild", 2010). `regex/pike.rs` keeps a list of threads at each position
//! and follows every one at every character. The list at a position is a
//! function of the list before it and the character read, so each list
//! met is kept as a state, and the state a character leads to is kept
//! beside it once found: a text read again through states met before
//! takes one step a character, whatever the program's size.
//!
//! A state is the instructions its threads wait at, in the order they are
//! preferred, before the jumps, forks and assertions at its position are
//! followed, and whether a match was found before it, after which no new
//! start is tried. Following them gives the threads that read a character
//! or match, cut after the first that matches, as every thread after it
//! is less preferred. The automaton so gives the end of the match
//! `pike.rs` finds, the leftmost and of those the preferred, though not
//! where it begins nor what its groups captured: `regex/bounded.rs` finds
//! those, reading no further than that end.
//!
//! `^` and `$` of the whole string hold only at its start and its end, so
//! the state the search begins in at the start is its own, and the end is
//! judged of the last state once the text is read. The wildcard of the
//! flag `s` matches any character but a CR before an LF [ISO/IEC
//! 9075-2:2023 §9.28 GR9a i], so a CR before an LF is read as a character
//! of its own, apart from every other CR. A program that asks more of a
//! position, `^` and `$` of each line, has no automaton here, nor has one
//! with a back-reference. Past `OCTETS_MOST` octets of states the automaton
//! gives up, and the search goes on without it.
//!
//! \[DESIGN\] (RULES.md D1312) A state no thread waits at leads, at every
//! character, to one no thread waits at, as a new start is tried at each
//! and led to none, so only the text's end could still match from it. If
//! the end cannot either, nothing can match any more and the scan stops
//! there. So a program anchored at the string's start, `^` of the whole
//! string, is given up at the first character no thread reads, where the
//! scan read every text with no match to its end.

use std::collections::BTreeMap;

use super::bounded::MARKS_OCTETS;
use super::compile::{Inst, Program};
use super::text::{Lines, Text};
use crate::error::Result;

/// \[DESIGN\] (RULES.md D1188) The most octets an automaton's states
/// take, their steps, their keys and their threads: as many as a search's
/// marks may (`bounded::MARKS_OCTETS`), so the two things a searcher keeps
/// are held to one bound, about a thousand states of a short program. An
/// automaton that still meets new states there is following its texts no
/// faster than the matchers do.
pub(super) const OCTETS_MOST: usize = MARKS_OCTETS;

/// The characters whose steps a state keeps: ASCII's, and after them a
/// CR before an LF.
const KEPT: usize = 129;

/// Where a state keeps the step of a CR before an LF.
const CR_BEFORE_LF: usize = 128;

/// A step not yet found.
const UNKNOWN: u32 = u32::MAX;

/// A step to no thread, a match found before: nothing more can match.
const DEAD: u32 = u32::MAX - 1;

/// What a state is: the instructions its threads wait at, in order;
/// whether a match was found before it; and whether it is the text's
/// start.
type Key = (Vec<u32>, bool, bool);

/// A state: its key, the threads that read a character or match once its
/// position's jumps are followed, whether the last of them matches, and
/// the state each ASCII character leads to, once found.
struct State {
    key: Key,
    threads: Vec<u32>,
    accepts: bool,
    next: Box<[u32; KEPT]>,
    /// Whether nothing can match from it (RULES.md D1312): no thread
    /// waits at it, and the text's end there matches nothing.
    barren: bool,
}

/// What a scan found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scan {
    /// No match.
    None,
    /// A match, the one preferred ending here.
    Ends(usize),
    /// Too many octets of states: not known.
    GaveUp,
}

/// Whether `program` has an automaton here: no back-reference and no
/// assertion of each line.
pub(super) fn fits(program: &Program) -> bool {
    program.insts.iter().all(|inst| match inst {
        Inst::LineStart(lines) | Inst::LineEnd(lines) => *lines == Lines::Whole,
        Inst::BackRef(_) => false,
        Inst::AnyButCrlf
        | Inst::Char(_)
        | Inst::Set(_)
        | Inst::Split(..)
        | Inst::Jump(_)
        | Inst::Save(_)
        | Inst::LoopEnter(_)
        | Inst::LoopCheck(_)
        | Inst::Match => true,
    })
}

/// The automaton of a program, its states made as texts are read, kept
/// from one text to the next.
#[derive(Default)]
pub(super) struct Dfa {
    states: Vec<State>,
    found: BTreeMap<Key, u32>,
    /// The octets its states take (`OCTETS_MOST`).
    held: usize,
    /// Scratch of `closed`: the work left and the instructions met.
    stack: Vec<usize>,
    seen: Vec<bool>,
    /// How many characters its scans read, counted for the tests.
    #[cfg(test)]
    read: usize,
}

impl Dfa {
    /// The octets its states take.
    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        self.held
    }

    /// The end of the match of `program` (`fits`) in `text` beginning at
    /// `from` or after, as `pike::search` finds it; of `first`, the first
    /// position a match is known at, enough to say there is one.
    ///
    /// # Errors
    ///
    /// As reading `text` fails.
    pub(super) fn scan<T: Text + ?Sized>(&mut self, program: &Program, text: &T, from: usize, first: bool) -> Result<Scan> {
        let len = text.len();
        let Some(mut state) = self.state(program, (Vec::new(), false, from == 0)) else { return Ok(Scan::GaveUp) };
        let mut last = None;
        let mut at = from;
        while at < len {
            if self.accepts(state) {
                last = Some(at);
                if first {
                    return Ok(Scan::Ends(at));
                }
            }
            let Some(c) = text.at(at)? else { break };
            #[cfg(test)]
            {
                self.read = self.read.saturating_add(1);
            }
            let crlf = c == '\r' && text.at(text.after(at))? == Some('\n');
            match self.step(program, state, (c, crlf)) {
                Some(DEAD) => return Ok(last.map_or(Scan::None, Scan::Ends)),
                Some(next) => state = next,
                None => return Ok(Scan::GaveUp),
            }
            at = text.after(at);
        }
        // The end of the text, where `$` holds: judged of the last state.
        let Some(key) = self.states.get(index(state)).map(|s| s.key.clone()) else { return Ok(Scan::GaveUp) };
        let matched = key.1 || last.is_some();
        let (_, accepts) = self.closed(program, (&key.0, matched), (key.2, true));
        Ok(if accepts { Scan::Ends(len) } else { last.map_or(Scan::None, Scan::Ends) })
    }

    /// Whether the threads of `state` match at its position.
    fn accepts(&self, state: u32) -> bool {
        self.states.get(index(state)).is_some_and(|s| s.accepts)
    }

    /// The state `c` leads to from `state`, `crlf` whether it is a CR
    /// before an LF: `DEAD` if no thread is left and no start is tried
    /// again; none past `OCTETS_MOST`.
    fn step(&mut self, program: &Program, state: u32, (c, crlf): (char, bool)) -> Option<u32> {
        let ascii = usize::try_from(u32::from(c)).ok().filter(|&at| at < CR_BEFORE_LF);
        let slot = if crlf { Some(CR_BEFORE_LF) } else { ascii };
        let from = self.states.get(index(state))?;
        if let Some(kept) = slot.and_then(|at| from.next.get(at)).copied().filter(|&next| next != UNKNOWN) {
            return Some(kept);
        }
        let reads = |pc: &&u32| match program.insts.get(index(**pc)) {
            Some(Inst::Char(want)) => c == *want,
            Some(Inst::Set(set)) => set.contains(c),
            Some(Inst::AnyButCrlf) => !crlf,
            Some(
                Inst::LineStart(_)
                | Inst::LineEnd(_)
                | Inst::Split(..)
                | Inst::Jump(_)
                | Inst::Save(_)
                | Inst::BackRef(_)
                | Inst::LoopEnter(_)
                | Inst::LoopCheck(_)
                | Inst::Match,
            )
            | None => false,
        };
        let kernel: Vec<u32> = from.threads.iter().filter(reads).map(|pc| pc.saturating_add(1)).collect();
        let matched = from.key.1 || from.accepts;
        let next = if kernel.is_empty() && matched { DEAD } else { self.live(program, (kernel, matched, false))? };
        if let Some(kept) = slot.and_then(|at| self.states.get_mut(index(state))?.next.get_mut(at)) {
            *kept = next;
        }
        Some(next)
    }

    /// \[DESIGN\] (RULES.md D1312) The state of `key`, or `DEAD` if
    /// nothing can match from it; none past `OCTETS_MOST`.
    fn live(&mut self, program: &Program, key: Key) -> Option<u32> {
        let at = self.state(program, key)?;
        let barren = self.states.get(index(at)).is_some_and(|state| state.barren);
        Some(if barren { DEAD } else { at })
    }

    /// The state of `key`, made if not met before; none past `OCTETS_MOST`.
    fn state(&mut self, program: &Program, key: Key) -> Option<u32> {
        if let Some(&at) = self.found.get(&key) {
            return Some(at);
        }
        let (threads, accepts) = self.closed(program, (&key.0, key.1), (key.2, false));
        // No thread waits and none will, so only the text's end is left to match.
        let barren = threads.is_empty() && !self.closed(program, (&key.0, key.1), (key.2, true)).1;
        // Its key is kept twice, by the state and by where it is found.
        let numbers = key.0.len().saturating_mul(2).saturating_add(threads.len());
        let octets = size_of::<[u32; KEPT]>().saturating_add(numbers.saturating_mul(size_of::<u32>()));
        self.held = self.held.saturating_add(octets);
        if self.held > OCTETS_MOST {
            return None;
        }
        let at = u32::try_from(self.states.len()).ok()?;
        self.found.insert(key.clone(), at);
        self.states.push(State { key, threads, accepts, next: Box::new([UNKNOWN; KEPT]), barren });
        Some(at)
    }

    /// The threads the instructions `kernel` lead to at a position, in the
    /// order they are preferred, a new start after them unless `matched`,
    /// cut after the first that matches; and whether one does. The
    /// position is the text's start if `start` and its end if `end`.
    fn closed(&mut self, program: &Program, (kernel, matched): (&[u32], bool), (start, end): (bool, bool)) -> (Vec<u32>, bool) {
        self.seen.clear();
        self.seen.resize(program.insts.len(), false);
        let mut threads = Vec::new();
        let restart = (!matched).then_some(0_u32);
        for &entry in kernel.iter().chain(&restart) {
            self.stack.push(index(entry));
            while let Some(pc) = self.stack.pop() {
                let Some(seen) = self.seen.get_mut(pc).filter(|seen| !**seen) else { continue };
                *seen = true;
                let after = pc.saturating_add(1);
                match program.insts.get(pc) {
                    Some(Inst::Jump(to)) => self.stack.push(*to),
                    Some(Inst::Split(first, second)) => self.stack.extend([*second, *first]),
                    Some(Inst::Save(_) | Inst::LoopEnter(_) | Inst::LoopCheck(_)) => self.stack.push(after),
                    Some(Inst::LineStart(_)) => self.stack.extend(start.then_some(after)),
                    Some(Inst::LineEnd(_)) => self.stack.extend(end.then_some(after)),
                    Some(Inst::Match) => {
                        threads.push(u32::try_from(pc).unwrap_or(u32::MAX));
                        self.stack.clear();
                        return (threads, true);
                    }
                    Some(Inst::Char(_) | Inst::Set(_) | Inst::AnyButCrlf | Inst::BackRef(_)) => {
                        threads.push(u32::try_from(pc).unwrap_or(u32::MAX));
                    }
                    None => {}
                }
            }
        }
        (threads, false)
    }
}

/// `n` as a place in a vector.
fn index(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

// Unit tests: the end found against `pike.rs`'s (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
