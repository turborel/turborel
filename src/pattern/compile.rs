//! [ISO/IEC 9075-2:2023 §7.9, §9.41 GR6] A `<row pattern>` compiled to the
//! instructions the search follows, each fork listing the preferred way
//! first, so a search that tries them in order meets the parenthesized
//! language's strings in the order of preferment.
//!
//! PERMUTE is written out as the alternation of its permutations [§7.9
//! SR21], a quantifier as a loop with a counter, never unrolled, so a large
//! bound costs no more code than a small one.

use super::{Pattern, beyond};
use crate::error::Result;

/// \[DESIGN\] The most patterns PERMUTE may permute. Its alternation has
/// n! alternatives, each a copy of every operand, so 8 gives 40,320 copies
/// and 9 would give 362,880, more than `MAX_INSTRUCTIONS` holds even of
/// single variables.
pub(super) const MAX_PERMUTE: usize = 8;

/// \[DESIGN\] The most instructions a program has, some tens of
/// megabytes, so nested PERMUTEs cannot exhaust memory. A PERMUTE of
/// `MAX_PERMUTE` variables takes about 400,000.
pub(super) const MAX_INSTRUCTIONS: usize = 1 << 19;

/// \[DESIGN\] How deep patterns may nest, so compiling, which recurses
/// once per level, cannot exhaust the stack.
pub(super) const MAX_DEPTH: usize = 256;

/// The length of the windows `next_permutation` reads, a pair of
/// neighbouring elements.
const PAIR: usize = 2;

/// One instruction of a program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Instr {
    /// [§9.41 GR8, GR9a] Maps the next row to `variable`, if there is one
    /// and the caller judges it may be; `excluded` if a `{- -}` holds it.
    Map {
        /// The primary row pattern variable.
        variable: usize,
        /// Between `{-` and `-}`.
        excluded: bool,
    },
    /// [§9.41 GR9b] `^`, which holds at the first row of the sequence alone.
    Start,
    /// [§9.41 GR9c] `$`, which holds past its last row alone.
    End,
    /// An alternation of no alternatives, whose language is empty.
    Fail,
    /// [§9.41 GR6e] The next instruction first, `later` on going back.
    Fork {
        /// Where the less preferred way begins.
        later: usize,
    },
    /// Goes on at the instruction given.
    Jump(usize),
    /// A quantifier begins, none of its iterations done.
    RepeatInit {
        /// Its counter.
        slot: usize,
    },
    /// [§9.41 GR6b, GR6c] Between iterations: another (the next
    /// instruction) or none (`exit`), the more preferred unless reluctant.
    RepeatLoop {
        /// Its counter.
        slot: usize,
        /// The least number of iterations.
        min: u64,
        /// The most, none if unbounded.
        max: Option<u64>,
        /// Fewer iterations preferred.
        reluctant: bool,
        /// Where the quantified pattern ends.
        exit: usize,
    },
    /// An iteration begins, counted and its first row kept.
    RepeatIter {
        /// Its counter.
        slot: usize,
    },
    /// [§9.41 GR6b i] An iteration ends; one past the first `min` that
    /// mapped no row is the last.
    RepeatNext {
        /// Its counter.
        slot: usize,
        /// The least number of iterations.
        min: u64,
        /// Where the choice of another iteration is made.
        again: usize,
        /// Where the quantified pattern ends.
        exit: usize,
    },
}

/// A program: its instructions, and how many counters they use.
#[derive(Debug, Clone)]
pub(super) struct Code {
    /// The instructions, the search matching on reaching their end.
    pub(super) instrs: Vec<Instr>,
    /// The number of quantifier counters.
    pub(super) slots: usize,
}

/// Where a pattern is compiled: how deep it nests and whether a `{- -}`
/// holds it.
#[derive(Debug, Clone, Copy)]
struct Within {
    depth: usize,
    excluded: bool,
}

impl Within {
    /// One level deeper, excluded if `excluded` or already.
    fn inner(self, excluded: bool) -> Result<Within> {
        let depth = self.depth.saturating_add(1);
        if depth > MAX_DEPTH {
            return Err(beyond(&format!("the row pattern nests more than {MAX_DEPTH} deep")));
        }
        Ok(Within { depth, excluded: self.excluded || excluded })
    }
}

/// `pattern` compiled.
///
/// # Errors
///
/// \[DESIGN\] SQLCODE -900 past `MAX_PERMUTE`, `MAX_INSTRUCTIONS` or
/// `MAX_DEPTH`.
pub(super) fn compile(pattern: &Pattern) -> Result<Code> {
    let mut code = Code { instrs: Vec::new(), slots: 0 };
    code.pattern(pattern, Within { depth: 0, excluded: false })?;
    Ok(code)
}

impl Code {
    /// Appends `instr`, returning where it is.
    fn emit(&mut self, instr: Instr) -> Result<usize> {
        let at = self.instrs.len();
        if at >= MAX_INSTRUCTIONS {
            return Err(beyond(&format!("the row pattern compiles to more than {MAX_INSTRUCTIONS} instructions")));
        }
        self.instrs.push(instr);
        Ok(at)
    }

    /// Points the fork or jump at `at` to `to`.
    fn patch(&mut self, at: usize, to: usize) {
        match self.instrs.get_mut(at) {
            Some(Instr::Fork { later }) => *later = to,
            Some(Instr::Jump(target)) => *target = to,
            Some(
                Instr::Map { .. }
                | Instr::Start
                | Instr::End
                | Instr::Fail
                | Instr::RepeatInit { .. }
                | Instr::RepeatLoop { .. }
                | Instr::RepeatIter { .. }
                | Instr::RepeatNext { .. },
            )
            | None => {}
        }
    }

    /// Compiles `pattern`, `within` where it is.
    fn pattern(&mut self, pattern: &Pattern, within: Within) -> Result<()> {
        match pattern {
            Pattern::Variable(variable) => self.emit(Instr::Map { variable: *variable, excluded: within.excluded }).map(drop),
            Pattern::Start => self.emit(Instr::Start).map(drop),
            Pattern::End => self.emit(Instr::End).map(drop),
            Pattern::Empty => Ok(()),
            Pattern::Concat(patterns) => {
                let inner = within.inner(false)?;
                patterns.iter().try_for_each(|p| self.pattern(p, inner))
            }
            Pattern::Alternation(patterns) => {
                let inner = within.inner(false)?;
                self.alternatives(patterns.len(), |code, k| patterns.get(k).map_or(Ok(()), |p| code.pattern(p, inner)))
            }
            Pattern::Repeat { pattern, min, max, reluctant } => {
                let quantifier = Quantifier { min: u64::from(*min), max: max.map(u64::from), reluctant: *reluctant };
                self.repeat(pattern, quantifier, within.inner(false)?)
            }
            Pattern::Excluded(pattern) => self.pattern(pattern, within.inner(true)?),
            Pattern::Permute(patterns) => self.permute(patterns, within.inner(false)?),
        }
    }

    /// [§9.41 GR6e] `count` alternatives, the earlier preferred, the k-th
    /// compiled by `each(self, k)`. None is an empty language.
    fn alternatives(&mut self, count: usize, mut each: impl FnMut(&mut Code, usize) -> Result<()>) -> Result<()> {
        if count == 0 {
            return self.emit(Instr::Fail).map(drop);
        }
        let mut jumps = Vec::new();
        for k in 0..count {
            let last = k.saturating_add(1) == count;
            let fork = if last { None } else { Some(self.emit(Instr::Fork { later: 0 })?) };
            each(self, k)?;
            if let Some(fork) = fork {
                jumps.push(self.emit(Instr::Jump(0))?);
                self.patch(fork, self.instrs.len());
            }
        }
        let end = self.instrs.len();
        for jump in jumps {
            self.patch(jump, end);
        }
        Ok(())
    }

    /// [§9.41 GR6b, GR6c] `pattern` repeated as `quantifier` says.
    fn repeat(&mut self, pattern: &Pattern, quantifier: Quantifier, within: Within) -> Result<()> {
        let slot = self.slots;
        self.slots = slot.saturating_add(1);
        self.emit(Instr::RepeatInit { slot })?;
        let Quantifier { min, max, reluctant } = quantifier;
        let again = self.emit(Instr::RepeatLoop { slot, min, max, reluctant, exit: 0 })?;
        self.emit(Instr::RepeatIter { slot })?;
        self.pattern(pattern, within)?;
        let next = self.emit(Instr::RepeatNext { slot, min, again, exit: 0 })?;
        let end = self.instrs.len();
        for at in [again, next] {
            if let Some(Instr::RepeatLoop { exit, .. } | Instr::RepeatNext { exit, .. }) = self.instrs.get_mut(at) {
                *exit = end;
            }
        }
        Ok(())
    }

    /// [§7.9 SR21] PERMUTE: the alternation of the concatenations of
    /// `patterns` in each order, the orders taken lexicographically.
    fn permute(&mut self, patterns: &[Pattern], within: Within) -> Result<()> {
        if patterns.len() > MAX_PERMUTE {
            return Err(beyond(&format!("PERMUTE permutes more than {MAX_PERMUTE} row patterns")));
        }
        let orders = permutations(patterns.len());
        self.alternatives(orders.len(), |code, k| {
            let order = orders.get(k).map_or(&[][..], Vec::as_slice);
            order.iter().filter_map(|&i| patterns.get(i)).try_for_each(|p| code.pattern(p, within))
        })
    }
}

/// A quantifier read as `{ min, max }`, reluctant or not [§7.9 SR19].
#[derive(Debug, Clone, Copy)]
struct Quantifier {
    min: u64,
    max: Option<u64>,
    reluctant: bool,
}

/// [§7.9 SR21] The permutations of `0..n`, in lexicographic order.
pub(super) fn permutations(n: usize) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..n).collect();
    let mut all = vec![order.clone()];
    while next_permutation(&mut order) {
        all.push(order.clone());
    }
    all
}

/// Turns `order` into the permutation that follows it lexicographically,
/// false if it is the last.
fn next_permutation(order: &mut [usize]) -> bool {
    let pairs = order.windows(PAIR).collect::<Vec<_>>();
    let Some(pivot) = pairs.iter().rposition(|w| w.first() < w.get(1)) else { return false };
    let Some(&low) = order.get(pivot) else { return false };
    let Some(swap) = order.iter().rposition(|&x| x > low) else { return false };
    order.swap(pivot, swap);
    if let Some(tail) = order.get_mut(pivot.saturating_add(1)..) {
        tail.reverse();
    }
    true
}
