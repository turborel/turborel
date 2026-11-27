//! \[DESIGN\] (RULES.md D1188) Where a program takes the rest of any text:
//! a greedy loop of the wildcard that matches every character, after
//! which nothing is asked but that the string ends there. `.*$` under
//! the flag `s` is one, and so is the end of `(.+)$`. From the loop's
//! first instruction the way preferred takes every character to the
//! string's end, where `$` holds, whatever the characters are, so a
//! matcher that goes back need not walk them (`regex/bounded.rs`).

use super::Inst;
use crate::regex::class::CharSet;
use crate::regex::text::Lines;

/// The last code point.
const LAST: u32 = 0x10_FFFF;

/// The loop of `insts` that takes the rest of any text, if they have one:
/// its first instruction, and the one after it, which the string's end
/// leads to.
pub(super) fn rest(insts: &[Inst]) -> Option<(usize, usize)> {
    let end = insts.iter().position(|inst| *inst == Inst::LineEnd(Lines::Whole))?;
    let straight = |inst: &Inst| matches!(inst, Inst::Save(_) | Inst::Match);
    if !insts.get(end.checked_add(1)?..)?.iter().all(straight) {
        return None;
    }
    let saves = insts.get(..end)?.iter().rev().take_while(|inst| matches!(inst, Inst::Save(_))).count();
    let after = end.checked_sub(saves)?;
    let Inst::Jump(first) = insts.get(after.checked_sub(1)?)? else { return None };
    let body = insts.get(first.checked_add(1)?..after.checked_sub(1)?)?;
    (insts.get(*first) == Some(&Inst::Split(first.saturating_add(1), after)) && every_character(body, first.saturating_add(1)))
        .then_some((*first, after))
}

/// Whether `body`, a loop's instructions from `at`, between its fork and
/// its jump back, match one character whatever it is: CRLF or else any
/// character but a CR before an LF, or a character of the set of all.
fn every_character(body: &[Inst], at: usize) -> bool {
    let all = CharSet::of(vec![(0, LAST)]);
    match body {
        [Inst::LoopEnter(_), Inst::Set(set), Inst::LoopCheck(_)] => *set == all,
        [
            Inst::LoopEnter(_),
            Inst::Split(crlf, single),
            Inst::Char('\r'),
            Inst::Char('\n'),
            Inst::Jump(check),
            Inst::AnyButCrlf,
            Inst::LoopCheck(_),
        ] => (*crlf, *single, *check) == (at.saturating_add(2), at.saturating_add(5), at.saturating_add(6)),
        _ => false,
    }
}
