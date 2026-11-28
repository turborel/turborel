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

use super::Inst;
use super::runs::taken_by;

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

// Unit tests: each of the three against the instructions themselves
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
