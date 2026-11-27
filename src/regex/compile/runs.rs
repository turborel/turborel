//! \[DESIGN\] (RULES.md D1190) Where a program runs through the characters
//! of a class. That is a greedy loop of one instruction that matches a
//! character, after which the next thing asked for is a character that
//! instruction never matches. `[^/]+/` has one, and so have `a*b` and
//! `[0-9]+x`. At
//! each position the loop's fork prefers another character of the loop,
//! and its other way, out of the loop, comes to that next instruction
//! with nothing read. While the loop's instruction matches the character
//! at a position, the next instruction does not, so the way out fails
//! there and need not be kept to try. Where the loop's instruction fails,
//! the way out is the only way. So a matcher that goes back follows the
//! loop to the first character it does not match and leaves it there,
//! with no fork kept and one mark a character (`regex/bounded.rs`).
//!
//! What follows the loop may first keep positions for groups, which
//! read nothing. Anything else before the next character, an assertion,
//! a fork, the match, leaves the loop a loop of forks, as does a
//! reluctant loop, a loop of more than one instruction, and the wildcard
//! that looks for an LF after a CR.

use super::Inst;
use crate::regex::class::CharSet;

/// The instructions of a loop of one instruction, from its fork to its
/// jump back. They are the fork, the position kept, the instruction, the
/// check and the jump.
const LOOP: usize = 5;

/// Of each instruction of `insts`, whether it is the fork of a loop the
/// program runs through (see the module).
pub(super) fn runs(insts: &[Inst]) -> Vec<bool> {
    (0..insts.len()).map(|at| run_at(insts, at)).collect()
}

/// Whether the instruction at `at` is the fork of a greedy loop of one
/// instruction that matches a character, the next character asked for
/// after it one that instruction never matches.
fn run_at(insts: &[Inst], at: usize) -> bool {
    let Some([Inst::Split(body, out), Inst::LoopEnter(_), taken, Inst::LoopCheck(_), Inst::Jump(back)]) =
        insts.get(at..at.saturating_add(LOOP))
    else {
        return false;
    };
    // Greedy, so its fork prefers the loop and leaves it for what is
    // after the jump back.
    let after = at.saturating_add(LOOP);
    if (*body, *out, *back) != (at.saturating_add(1), after, at) {
        return false;
    }
    let next = insts.iter().skip(after).find(|inst| !matches!(inst, Inst::Save(_)));
    next.is_some_and(|next| apart(taken, next))
}

/// Whether `taken` and `next` each match one character by what it is
/// alone, and no character both.
fn apart(taken: &Inst, next: &Inst) -> bool {
    taken_by(taken).zip(taken_by(next)).is_some_and(|(ours, theirs)| ours.apart(&theirs))
}

/// The characters `inst` matches, if it matches one character by what
/// that character is alone, reading none beside it.
fn taken_by(inst: &Inst) -> Option<CharSet> {
    match inst {
        Inst::Char(c) => Some(CharSet::single(*c)),
        Inst::Set(set) => Some(set.clone()),
        Inst::AnyButCrlf
        | Inst::LineStart(_)
        | Inst::LineEnd(_)
        | Inst::Split(..)
        | Inst::Jump(_)
        | Inst::Save(_)
        | Inst::BackRef(_)
        | Inst::LoopEnter(_)
        | Inst::LoopCheck(_)
        | Inst::Match => None,
    }
}
