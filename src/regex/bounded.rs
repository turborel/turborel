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
//!
//! \[DESIGN\] (RULES.md D1311) A step does its work once. Only an
//! instruction two ways can meet at is marked (`compile/steps.rs`), as a
//! way reaches any other from the one before it alone. Of a text read by
//! its octets, a character of ASCII is tested by the instruction's table
//! of them. And a loop the program runs through (RULES.md D1190) finds
//! where it ends in one pass over the octets, by a search for the one
//! octet that ends it where there is one, and marks the positions it
//! passed a word at a time.

use super::compile::{Inst, LOOP_TAKES, MAX_INSTRUCTIONS, Program, holds};
use super::pike::{Slots, Thread};
use super::text::{Lines, PAST_ASCII, Text, line_end, line_start, one};
use crate::error::Result;
use crate::kernels::portable::find::find_bytes;

/// \[DESIGN\] (RULES.md D1188) The most marks a search keeps, a bit each:
/// as many octets as the threads of the matcher of threads take for the
/// longest program, its two lists of `MAX_INSTRUCTIONS` threads each,
/// what they captured not counted. So a search here never holds more
/// than a search there may, and the budget is of what a search holds: a
/// sweep of `bench/regex.rs` found no length of text within it at which
/// the matcher of threads ran the text faster.
pub(super) const MARKS_MOST: usize = MARKS_OCTETS * OCTET;

/// The octets of `MARKS_MOST` marks.
pub(super) const MARKS_OCTETS: usize = LISTS * MAX_INSTRUCTIONS * size_of::<Thread>();

/// The lists of threads the matcher of threads keeps, this position's and
/// the next's.
const LISTS: usize = 2;

/// The bits of an octet.
const OCTET: usize = 8;

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

/// \[DESIGN\] (RULES.md D1188) What a search keeps while it runs, kept by
/// a searcher from one search to the next (`regex/searcher.rs`), so a
/// search of a text takes no memory anew once one as long was searched.
#[derive(Default)]
pub(super) struct Scratch {
    marks: Vec<u64>,
    slots: Slots,
    stack: Vec<Job>,
    /// How many instructions the searches followed, counted for the tests
    /// of the bound.
    #[cfg(test)]
    pub(super) steps: usize,
}

impl Scratch {
    /// The slots of the match the last search found, which say where
    /// its groups are.
    pub(super) fn slots(&self) -> &[Option<usize>] {
        &self.slots
    }

    /// Keeps `slots`, those of a match another matcher found, as its
    /// own search would have left them.
    pub(super) fn put(&mut self, slots: Slots) {
        self.slots = slots;
    }
}

/// What a search reads and keeps.
struct Search<'a, T: Text + ?Sized> {
    program: &'a Program,
    text: &'a T,
    /// The text's octets, if it is read by them (`Text::octets`).
    octets: Option<&'a [u8]>,
    from: usize,
    positions: usize,
    capture: bool,
    kept: &'a mut Scratch,
}

/// Whether every match of `program` begins at the text's start: its first
/// instruction after the one that keeps where the match begins is `^` of
/// the whole string, so a way from any later start fails there.
fn anchored(program: &Program) -> bool {
    matches!(program.insts.get(1), Some(Inst::LineStart(Lines::Whole)))
}

/// The first match of `program`, which has no back-reference, in `text`
/// beginning at `from` or after, of `positions` positions (`positions`),
/// no character read past them: each slot of where its groups are if
/// `capture`, else none of them, as `pike::search` gives them. Of fewer
/// positions than the text has, the match is one known to end within
/// them (`regex/dfa.rs`), so a way that reads past them is not it. A
/// program whose matches all begin at the text's start is tried from
/// there alone. What the search keeps is `kept`'s, left for the next,
/// and the slots of a match found are left there too (`Scratch::slots`),
/// so a match takes no memory of its own (RULES.md D1189).
///
/// # Errors
///
/// As reading `text` fails.
pub(super) fn search<T: Text + ?Sized>(
    program: &Program,
    text: &T,
    (from, positions): (usize, usize),
    capture: bool,
    kept: &mut Scratch,
) -> Result<bool> {
    let first_alone = anchored(program);
    kept.marks.clear();
    kept.marks.resize(program.insts.len().saturating_mul(positions).div_ceil(WORD), 0);
    kept.slots.clear();
    kept.slots.resize(if capture { program.slots } else { 0 }, None);
    kept.stack.clear();
    let mut s = Search { program, text, octets: text.octets(), from, positions, capture, kept };
    let last = from.saturating_add(positions.saturating_sub(1));
    let mut start = from;
    while start <= last && (start == 0 || !first_alone) {
        s.kept.slots.fill(None);
        s.kept.stack.push(Job::Try(0, start));
        while let Some(job) = s.kept.stack.pop() {
            match job {
                Job::Restore(slot, old) => s.set(slot, old),
                Job::Try(pc, at) => {
                    if s.run(pc, at)? {
                        return Ok(true);
                    }
                }
            }
        }
        start = text.after(start);
    }
    Ok(false)
}

impl<T: Text + ?Sized> Search<'_, T> {
    fn set(&mut self, slot: usize, value: Option<usize>) {
        if let Some(s) = self.kept.slots.get_mut(slot) {
            *s = value;
        }
    }

    /// Marks instruction `pc` at `at`: whether it was not marked before.
    /// One past the marks counts as marked, so it is not followed.
    fn mark(&mut self, pc: usize, at: usize) -> bool {
        let bit = self.bit(pc, at);
        let (word, mask) =
            (bit.checked_div(WORD).unwrap_or(usize::MAX), 1_u64.checked_shl(u32::try_from(bit % WORD).unwrap_or(0)).unwrap_or(0));
        let Some(marks) = self.kept.marks.get_mut(word) else { return false };
        let fresh = *marks & mask == 0;
        *marks |= mask;
        fresh
    }

    /// Of the loop that takes the rest of any text (`compile/rest.rs`), if
    /// `pc` is its first instruction: the instruction after it and the
    /// string's end, where the way preferred from here goes, whatever the
    /// characters between, so none of them is walked; no end if the match
    /// is known to end before the string does.
    fn rest(&self, pc: usize) -> Option<(usize, Option<usize>)> {
        let (_, after) = self.program.passed.rest.filter(|(first, _)| *first == pc)?;
        let end = self.text.len();
        Some((after, (end.saturating_sub(self.from) < self.positions).then_some(end)))
    }

    /// Where the search goes on from the fork at `pc`, met at `at`, that
    /// prefers `first` to `second`. Of a loop the program runs through
    /// (RULES.md D1190) that is past the loop, at `second`, and nowhere
    /// if the loop was passed there before. Of any other fork it is
    /// `first`, the other way kept to try.
    fn forked(&mut self, (pc, at): (usize, usize), (first, second): (usize, usize)) -> Result<Option<(usize, usize)>> {
        if !self.program.passed.runs.get(pc).copied().unwrap_or(false) {
            self.kept.stack.push(Job::Try(second, at));
            return Ok(Some((first, at)));
        }
        Ok(self.through(pc, at)?.map(|left| (second, left)))
    }

    /// \[DESIGN\] (RULES.md D1190) Of the loop whose fork is `pc`, one
    /// the program runs through (`compile/runs.rs`), met at `at`, the
    /// position it is left at, which is the first whose character its
    /// instruction does not match, or the last the search reads to. No
    /// fork is kept on the way, as leaving the loop any earlier fails at
    /// once. The fork is marked at each position passed, as following
    /// the forks marks it, and none is given if one of them was marked
    /// before, since the way from there was followed already. Of a text
    /// read by its octets the characters of ASCII are passed in one pass
    /// first (`passed`, RULES.md D1311).
    fn through(&mut self, pc: usize, met: usize) -> Result<Option<usize>> {
        let taken_at = pc.saturating_add(LOOP_TAKES);
        let Some(taken) = self.program.insts.get(taken_at) else { return Ok(Some(met)) };
        let mut at = self.passed(pc, met);
        let (first, end) = (self.bit(pc, self.text.after(met)), self.bit(pc, at).saturating_add(1));
        if at > met && !marked(&mut self.kept.marks, first, end) {
            return Ok(None);
        }
        loop {
            let within = at.saturating_sub(self.from).saturating_add(1) < self.positions;
            if !within || !self.takes((taken_at, taken), at)? {
                return Ok(Some(at));
            }
            at = self.text.after(at);
            if !self.mark(pc, at) {
                return Ok(None);
            }
        }
    }

    /// \[DESIGN\] (RULES.md D1311) Of the loop whose fork is `pc`, met at
    /// `met` in a text read by its octets, the position it comes to in
    /// one pass over them: the first octet that ends it, if one octet
    /// alone does (`Passed::ends`), found a word at a time; else the
    /// first octet that is no character of ASCII its instruction matches.
    /// It is `met` of any other text, and never past the last position
    /// the search reads to.
    fn passed(&self, pc: usize, met: usize) -> usize {
        let last = self.from.saturating_add(self.positions.saturating_sub(1));
        let Some(octets) = self.octets.and_then(|octets| octets.get(met..last)) else { return met };
        let passed = match (self.program.passed.ends.get(pc), self.program.passed.ascii.get(pc.saturating_add(LOOP_TAKES))) {
            (Some(Some(end)), _) => find_bytes(octets, std::slice::from_ref(end)),
            (Some(None) | None, Some(table)) => octets.iter().position(|octet| *octet >= PAST_ASCII || !holds(table, *octet)),
            (Some(None) | None, None) => Some(0),
        };
        met.saturating_add(passed.unwrap_or(octets.len()))
    }

    /// The mark of instruction `pc` at `at`, as a bit's place among the
    /// marks.
    fn bit(&self, pc: usize, at: usize) -> usize {
        pc.saturating_mul(self.positions).saturating_add(at.saturating_sub(self.from))
    }

    /// \[DESIGN\] (RULES.md D1311) Whether `inst`, the instruction at
    /// `pc`, one that matches a character, matches the one at `at`: of a
    /// text read by its octets, a character of ASCII by the instruction's
    /// table of them, if it matches by what the character is alone.
    #[inline]
    fn takes(&self, (pc, inst): (usize, &Inst), at: usize) -> Result<bool> {
        let tabled = matches!(inst, Inst::Char(_) | Inst::Set(_));
        let ascii = self.octets.and_then(|octets| octets.get(at)).copied().filter(|octet| tabled && *octet < PAST_ASCII);
        match (ascii, self.program.passed.ascii.get(pc)) {
            (Some(octet), Some(table)) => Ok(holds(table, octet)),
            (Some(_), None) | (None, _) => one(inst, self.text, at),
        }
    }

    /// \[DESIGN\] (RULES.md D1311) Whether the way that comes to
    /// instruction `pc` at `at` goes on. It does not where two ways can
    /// meet at the instruction and one came there at that position
    /// before, which the mark says. An instruction no two ways meet at is
    /// not marked.
    fn arrives(&mut self, pc: usize, at: usize) -> bool {
        let meets = self.program.passed.meets.get(pc).copied().unwrap_or(true);
        let goes_on = !meets || self.mark(pc, at);
        #[cfg(test)]
        {
            self.kept.steps = self.kept.steps.saturating_add(usize::from(goes_on));
        }
        goes_on
    }

    /// Follows one way from `pc` at `at` until it matches, true, or fails
    /// or meets an instruction at a position met before, false, forks
    /// kept to try.
    fn run(&mut self, mut pc: usize, mut at: usize) -> Result<bool> {
        loop {
            if !self.arrives(pc, at) {
                return Ok(false);
            }
            let program = self.program;
            if let Some((after, end)) = self.rest(pc) {
                let Some(end) = end else { return Ok(false) };
                (pc, at) = (after, end);
                continue;
            }
            let Some(inst) = program.insts.get(pc) else { return Ok(false) };
            let after = pc.saturating_add(1);
            match inst {
                Inst::Match => return Ok(true),
                Inst::Jump(to) => pc = *to,
                Inst::Split(first, second) => match self.forked((pc, at), (*first, *second))? {
                    Some(next) => (pc, at) = next,
                    None => return Ok(false),
                },
                Inst::Save(slot) => {
                    if self.capture {
                        let old = self.kept.slots.get(*slot).copied().flatten();
                        self.kept.stack.push(Job::Restore(*slot, old));
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
                    if !within || !self.takes((pc, inst), at)? {
                        return Ok(false);
                    }
                    (pc, at) = (after, self.text.after(at));
                }
                // No program with one runs here (`Regex::matcher`).
                Inst::BackRef(_) => return Ok(false),
            }
        }
    }
}

/// \[DESIGN\] (RULES.md D1311) Sets the bits `first` up to `end`, not
/// `end` itself, of `marks`: whether none of them was set before. If one
/// was, the bits before the first such are set and no other, as marking
/// them one by one up to it would leave them. A bit past the marks counts
/// as set.
fn marked(marks: &mut [u64], first: usize, end: usize) -> bool {
    let mut bit = first;
    while bit < end {
        let word = bit.checked_div(WORD).unwrap_or(usize::MAX);
        let start = word.saturating_mul(WORD);
        let to = end.saturating_sub(start).min(WORD);
        let wanted = ones(to) & !ones(bit % WORD);
        let Some(held) = marks.get_mut(word) else { return false };
        let met = *held & wanted;
        if met != 0 {
            *held |= wanted & ones(usize::try_from(met.trailing_zeros()).unwrap_or(0));
            return false;
        }
        *held |= wanted;
        bit = start.saturating_add(to);
    }
    true
}

/// A word whose lowest `n` bits are set, all of them of `WORD` or more.
fn ones(n: usize) -> u64 {
    u32::try_from(n).ok().and_then(|n| 1_u64.checked_shl(n)).map_or(u64::MAX, |bit| bit.wrapping_sub(1))
}

// Unit tests: the match found against `pike.rs`'s (RULES.md R-TEST-1).
#[cfg(test)]
pub(super) mod tests;
