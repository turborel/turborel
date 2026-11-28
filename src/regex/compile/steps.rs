//! \[DESIGN\] (RULES.md D1311) What a program knows of each instruction so
//! that a matcher that goes back does a step's work once a step
//! (`regex/bounded.rs`).
//!
//! Where two ways can meet. A way comes to an instruction from the one
//! before it, at one position, unless a fork or a jump leads to it. So an
//! instruction and a position are reached twice only if an instruction a
//! fork or a jump leads to was reached twice at one position before them
//! on that straight run. Marking those instructions alone, with the
//! first of the program, which every start begins at, and the one after
//! the loop that takes the rest of a text (`compile/rest.rs`), which
//! every position leads to, gives up every way a mark of each instruction
//! gave up, at the instruction two ways first share.
//!
//! The characters of ASCII an instruction matches, a bit each, so one is
//! tested by its octet with no search of a set's ranges.
//!
//! The one octet that ends a loop run through (`compile/runs.rs`) whose
//! instruction matches every character but one of ASCII, as `[^/]+` does.
//! Its end in a text read by its octets is the first such octet, which a
//! search of the octets finds a word at a time, as no octet of a longer
//! character is one of ASCII.

//!
//! \[DESIGN\] (RULES.md D1312) The straight runs: instructions one after
//! another that each match one given character of ASCII, with no way in
//! but at the run's first. A text read by its octets is compared with a
//! run's octets at once.
//!
//! And whether the program has few ways. A program whose every match
//! begins at the string's start, and whose loops are all run through or
//! skipped as the rest of the text, forks into two ways at each fork left
//! and into no more, each way a straight walk. While two to the power of
//! its forks, times its loops and one, is within its instructions, all
//! its ways together follow fewer instructions than the program has
//! times the program and the text together, with no mark kept.

use super::Inst;
use super::runs::{LOOP, taken_by};
use crate::regex::text::Lines;

/// The characters of ASCII.
const ASCII: u8 = 128;

/// The bits of a word of a table.
const WORD: u8 = 64;

/// The characters of ASCII an instruction matches, a bit each, the first
/// 64 in the first word.
pub(in crate::regex) type Table = [u64; 2];

/// Whether the table `table` holds the octet `octet`, one of ASCII.
#[inline]
pub(in crate::regex) fn holds(table: &Table, octet: u8) -> bool {
    let word = table.get(usize::from(octet.checked_div(WORD).unwrap_or(0))).copied().unwrap_or(0);
    word.checked_shr(u32::from(octet % WORD)).unwrap_or(0) & 1 == 1
}

/// Of each instruction of `insts`, whether two ways can meet at it (see
/// the module): the first, each a fork or a jump leads to, and `after`,
/// the one after the loop that takes the rest of a text, if there is one.
pub(super) fn meets(insts: &[Inst], rest: Option<(usize, usize)>) -> Vec<bool> {
    let mut meets = vec![false; insts.len()];
    let led_to = insts.iter().flat_map(|inst| match inst {
        Inst::Split(first, second) => [Some(*first), Some(*second)],
        Inst::Jump(to) => [Some(*to), None],
        Inst::Char(_)
        | Inst::Set(_)
        | Inst::AnyButCrlf
        | Inst::LineStart(_)
        | Inst::LineEnd(_)
        | Inst::Save(_)
        | Inst::BackRef(_)
        | Inst::LoopEnter(_)
        | Inst::LoopCheck(_)
        | Inst::Match => [None, None],
    });
    for pc in led_to.flatten().chain([0]).chain(rest.map(|(_, after)| after)) {
        if let Some(met) = meets.get_mut(pc) {
            *met = true;
        }
    }
    meets
}

/// Of each instruction of `insts` that matches a character by what it is
/// alone, the characters of ASCII it matches; of any other, none.
pub(super) fn tables(insts: &[Inst]) -> Vec<Table> {
    insts
        .iter()
        .map(|inst| {
            let mut table = [0_u64; 2];
            let Some(set) = taken_by(inst) else { return table };
            for octet in (0..ASCII).filter(|octet| set.contains(char::from(*octet))) {
                if let Some(word) = table.get_mut(usize::from(octet.checked_div(WORD).unwrap_or(0))) {
                    *word |= 1_u64.checked_shl(u32::from(octet % WORD)).unwrap_or(0);
                }
            }
            table
        })
        .collect()
}

/// Of each instruction of `insts`, the one octet of ASCII that ends the
/// loop it is the fork of, if `runs` says the program runs through that
/// loop and the loop's instruction, `ahead` after the fork, matches
/// every character but that one.
pub(super) fn ends(insts: &[Inst], runs: &[bool], ahead: usize) -> Vec<Option<u8>> {
    (0..insts.len())
        .map(|pc| {
            let taken = insts.get(pc.checked_add(ahead)?).filter(|_| runs.get(pc) == Some(&true))?;
            let left_out = taken_by(taken)?.complement();
            let (first, last) = left_out.ranges().first().copied().filter(|_| left_out.ranges().len() == 1)?;
            u8::try_from(first).ok().filter(|octet| first == last && *octet < ASCII)
        })
        .collect()
}

/// \[DESIGN\] (RULES.md D1312) Of each instruction of `insts`, the
/// character of ASCII it matches as an octet, 0 of any other; and how
/// many instructions from it on each match one such character with no
/// way in after the first, which `meets` tells: its straight run, 0 or 1
/// of an instruction that begins none.
pub(super) fn straight(insts: &[Inst], meets: &[bool]) -> (Vec<u8>, Vec<u8>) {
    let octet = |inst: &Inst| if let Inst::Char(c) = inst { u8::try_from(*c).ok().filter(|octet| *octet < ASCII) } else { None };
    let octets: Vec<u8> = insts.iter().map(|inst| octet(inst).unwrap_or(0)).collect();
    let mut runs = vec![0_u8; insts.len()];
    for pc in (0..insts.len()).rev() {
        let after = pc.checked_add(1).filter(|after| meets.get(*after) == Some(&false)).and_then(|after| runs.get(after)).copied();
        if let (Some(_), Some(run)) = (insts.get(pc).and_then(octet), runs.get_mut(pc)) {
            *run = after.unwrap_or(0).saturating_add(1);
        }
    }
    (octets, runs)
}

/// \[DESIGN\] (RULES.md D1312) Whether the program `insts` has few ways
/// (see the module): every match begins at the string's start, `runs`
/// and `rest` tell its only loops, and two to the power of its other
/// forks, times its loops and one, is within its instructions.
pub(super) fn few_ways(insts: &[Inst], runs: &[bool], rest: Option<(usize, usize)>) -> bool {
    let skipped = |pc: usize| rest.is_some_and(|(first, after)| first <= pc && pc < after);
    let run_through = |pc: usize| (0..LOOP).any(|back| pc.checked_sub(back).is_some_and(|fork| runs.get(fork) == Some(&true)));
    let mut forks = 0_u32;
    for (pc, inst) in insts.iter().enumerate().filter(|(pc, _)| !skipped(*pc) && !run_through(*pc)) {
        match inst {
            Inst::Split(..) => forks = forks.saturating_add(1),
            Inst::Jump(to) => {
                if *to <= pc {
                    return false;
                }
            }
            Inst::BackRef(_) => return false,
            Inst::Char(_)
            | Inst::Set(_)
            | Inst::AnyButCrlf
            | Inst::LineStart(_)
            | Inst::LineEnd(_)
            | Inst::Save(_)
            | Inst::LoopEnter(_)
            | Inst::LoopCheck(_)
            | Inst::Match => {}
        }
    }
    let anchored = matches!(insts.get(1), Some(Inst::LineStart(Lines::Whole)));
    let loops = runs.iter().filter(|run| **run).count().saturating_add(1);
    let ways = 1_usize.checked_shl(forks).and_then(|ways| ways.checked_mul(loops));
    anchored && ways.is_some_and(|ways| ways <= insts.len())
}

// Unit tests: each of the three against the instructions themselves
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
