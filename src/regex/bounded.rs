//! \[DESIGN\] (RULES.md D1188) The matcher of a program with no
//! back-reference over a text short enough: it tries each way in the order
//! preferred, going back to the last fork on failing, as a backtracking
//! matcher does, but follows an instruction at a position once in a
//! search, whichever way led there, so it takes time at most the product
//! of the text's length and the program's, as `regex/pike.rs` does, with
//! one thread, no list of threads and no copy of what a thread captured.
//!
//! A way that reaches an instruction at a position reached before is
//! given up. That is right because whether a match is found from an
//! instruction at a position depends on those two alone: on nothing
//! captured, and not on where the search began. The way that reached them
//! first is preferred to every later one, and it found no match from
//! there, or the search would have ended. So the marks are kept from one
//! starting position to the next, and a search of every start is still
//! bounded by the one product. A back-reference breaks this, as what it
//! matches depends on what was captured, so a program with one never runs
//! here (`Regex::matcher`).
//!
//! The match found is the one `pike.rs` finds: the leftmost, and of those
//! the one alternation's order and the quantifiers' greed prefer. An
//! iteration that matches nothing comes back to its fork at the position
//! it left, which is marked, so it is given up as `pike.rs` gives it up.
//!
//! The marks are a bit of each instruction at each position. A text and a
//! program whose product is past `MARKS_MOST` bits run on `pike.rs`.

use super::compile::{Inst, Program};
use super::pike::Slots;
use super::text::{Text, line_end, line_start, one};
use crate::error::Result;

/// \[DESIGN\] (RULES.md D1188) The most marks a search keeps, a bit each,
/// half a mebibyte of them. From a sweep of `bench/regex.rs` at five
/// budgets from `1 << 14`: at every length of text a budget admitted, the
/// matcher that marks ran several times as fast as the matcher of threads
/// ran the same text, to the largest budget swept, this one. So the budget
/// bounds what a search holds, not where the matcher stops paying.
pub(super) const MARKS_MOST: usize = 1 << 22;

/// The bits of a word of marks.
const WORD: usize = 64;

/// Work left: a way to try, or a slot to put back on going back.
enum Job {
    Try(usize, usize),
    Restore(usize, Option<usize>),
}

/// How many positions a search from `from` reads that reads no character
/// at `end` or after, the text's length or where its match is known to
/// end, if `program`'s marks of them are within `MARKS_MOST`; none if they
/// are more.
pub(super) fn positions(program: &Program, end: usize, from: usize) -> Option<usize> {
    let positions = end.checked_sub(from)?.checked_add(1)?;
    (program.insts.len().checked_mul(positions)? <= MARKS_MOST).then_some(positions)
}

/// What a search reads and keeps.
struct Search<'a, T: Text + ?Sized> {
    program: &'a Program,
    text: &'a T,
    from: usize,
    positions: usize,
    capture: bool,
    marks: Vec<u64>,
    slots: Slots,
    stack: Vec<Job>,
}

/// The first match of `program`, which has no back-reference, in `text`
/// beginning at `from` or after, of `positions` positions (`positions`),
/// no character read past them: each slot of where its groups are if
/// `capture`, else none of them, as `pike::search` gives them. Of fewer
/// positions than the text has, the match is one known to end within
/// them (`regex/dfa.rs`), so a way that reads past them is not it.
///
/// # Errors
///
/// As reading `text` fails.
pub(super) fn search<T: Text + ?Sized>(
    program: &Program,
    text: &T,
    (from, positions): (usize, usize),
    capture: bool,
) -> Result<Option<Slots>> {
    let words = program.insts.len().saturating_mul(positions).div_ceil(WORD);
    let slots = vec![None; if capture { program.slots } else { 0 }];
    let mut s = Search { program, text, from, positions, capture, marks: vec![0; words], slots, stack: Vec::new() };
    for start in from..from.saturating_add(positions) {
        s.slots.fill(None);
        s.stack.push(Job::Try(0, start));
        while let Some(job) = s.stack.pop() {
            match job {
                Job::Restore(slot, old) => s.set(slot, old),
                Job::Try(pc, at) => {
                    if s.run(pc, at)? {
                        return Ok(Some(s.slots));
                    }
                }
            }
        }
    }
    Ok(None)
}

impl<T: Text + ?Sized> Search<'_, T> {
    fn set(&mut self, slot: usize, value: Option<usize>) {
        if let Some(s) = self.slots.get_mut(slot) {
            *s = value;
        }
    }

    /// Marks instruction `pc` at `at`: whether it was not marked before.
    /// One past the marks counts as marked, so it is not followed.
    fn mark(&mut self, pc: usize, at: usize) -> bool {
        let bit = pc.saturating_mul(self.positions).saturating_add(at.saturating_sub(self.from));
        let (word, mask) =
            (bit.checked_div(WORD).unwrap_or(usize::MAX), 1_u64.checked_shl(u32::try_from(bit % WORD).unwrap_or(0)).unwrap_or(0));
        let Some(marks) = self.marks.get_mut(word) else { return false };
        let fresh = *marks & mask == 0;
        *marks |= mask;
        fresh
    }

    /// Follows one way from `pc` at `at` until it matches, true, or fails
    /// or meets an instruction at a position met before, false, forks
    /// kept to try.
    fn run(&mut self, mut pc: usize, mut at: usize) -> Result<bool> {
        loop {
            if !self.mark(pc, at) {
                return Ok(false);
            }
            let program = self.program;
            let Some(inst) = program.insts.get(pc) else { return Ok(false) };
            let after = pc.saturating_add(1);
            match inst {
                Inst::Match => return Ok(true),
                Inst::Jump(to) => pc = *to,
                Inst::Split(first, second) => {
                    self.stack.push(Job::Try(*second, at));
                    pc = *first;
                }
                Inst::Save(slot) => {
                    if self.capture {
                        let old = self.slots.get(*slot).copied().flatten();
                        self.stack.push(Job::Restore(*slot, old));
                        self.set(*slot, Some(at));
                    }
                    pc = after;
                }
                // An iteration that matches nothing comes back to its fork
                // at a position marked, so needs no check here.
                Inst::LoopEnter(_) | Inst::LoopCheck(_) => pc = after,
                Inst::LineStart(lines) => {
                    if !line_start(self.text, at, *lines)? {
                        return Ok(false);
                    }
                    pc = after;
                }
                Inst::LineEnd(lines) => {
                    if !line_end(self.text, at, *lines)? {
                        return Ok(false);
                    }
                    pc = after;
                }
                Inst::Char(_) | Inst::Set(_) | Inst::AnyButCrlf => {
                    let within = at.saturating_sub(self.from).saturating_add(1) < self.positions;
                    if !within || !one(inst, self.text, at)? {
                        return Ok(false);
                    }
                    (pc, at) = (after, at.saturating_add(1));
                }
                // No program with one runs here (`Regex::matcher`).
                Inst::BackRef(_) => return Ok(false),
            }
        }
    }
}

// Unit tests: the match found against `pike.rs`'s (RULES.md R-TEST-1).
#[cfg(test)]
pub(super) mod tests;
