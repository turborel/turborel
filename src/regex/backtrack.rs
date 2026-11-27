//! \[DESIGN\] The matcher of a program with a back-reference, which no
//! automaton matches: it tries each way in the order preferred, going back
//! to the last fork on failing. Its time can grow exponentially with the
//! string, so it stops past `MAX_STEPS` (RULES.md D61, D18).

use super::compile::{Inst, Program};
use super::pike::Slots;
use super::text::{Text, line_end, line_start, one};
use super::unicode::Table;
use crate::error::Result;

/// [DESIGN] The most instructions a search with back-references follows,
/// about a tenth of a second's work (CONFORMANCE.md).
pub(super) const MAX_STEPS: u64 = 10_000_000;

/// Work left: a way to try, or a slot to put back on going back.
enum Job {
    Try(usize, usize),
    Restore(usize, Option<usize>),
}

/// What a search reads and keeps.
struct Search<'a, T: Text + ?Sized> {
    program: &'a Program,
    text: &'a T,
    caseless: Option<&'a Table>,
    steps: u64,
    slots: Slots,
    stack: Vec<Job>,
}

/// The first match of `program` in `text` beginning at `from` or after, a
/// back-reference matching case variants too if `caseless` gives the table
/// of them. It reads `text` where it goes, going back too.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_STEPS`; as reading `text` fails.
pub(super) fn search<T: Text + ?Sized>(program: &Program, text: &T, from: usize, caseless: Option<&Table>) -> Result<Option<Slots>> {
    let all = program.slots.saturating_add(program.loops);
    let mut s = Search { program, text, caseless, steps: 0, slots: vec![None; all], stack: Vec::new() };
    let mut start = from;
    while start <= text.len() {
        s.slots.fill(None);
        s.stack.clear();
        s.stack.push(Job::Try(0, start));
        while let Some(job) = s.stack.pop() {
            match job {
                Job::Restore(slot, old) => s.set(slot, old),
                Job::Try(pc, at) => {
                    if s.run(pc, at)? {
                        return Ok(Some(s.slots.get(..program.slots).map(<[_]>::to_vec).unwrap_or_default()));
                    }
                }
            }
        }
        start = text.after(start);
    }
    Ok(None)
}

impl<T: Text + ?Sized> Search<'_, T> {
    fn set(&mut self, slot: usize, value: Option<usize>) {
        if let Some(s) = self.slots.get_mut(slot) {
            *s = value;
        }
    }

    /// Keeps `at` in `slot`, to be put back on going back.
    fn keep(&mut self, slot: usize, at: usize) {
        let old = self.slots.get(slot).copied().flatten();
        self.stack.push(Job::Restore(slot, old));
        self.set(slot, Some(at));
    }

    /// Follows one way from `pc` at `at` until it matches, true, or fails,
    /// false, forks kept to try.
    fn run(&mut self, mut pc: usize, mut at: usize) -> Result<bool> {
        loop {
            self.steps = self.steps.saturating_add(1);
            if self.steps > MAX_STEPS {
                return Err(super::beyond(&format!("matching with back-references took more than {MAX_STEPS} steps")));
            }
            let program = self.program;
            let Some(inst) = program.insts.get(pc) else { return Ok(false) };
            match inst {
                Inst::Match => return Ok(true),
                Inst::Jump(to) => pc = *to,
                Inst::Split(first, second) => {
                    self.stack.push(Job::Try(*second, at));
                    pc = *first;
                }
                Inst::Save(_)
                | Inst::LoopEnter(_)
                | Inst::LoopCheck(_)
                | Inst::LineStart(_)
                | Inst::LineEnd(_)
                | Inst::BackRef(_)
                | Inst::Char(_)
                | Inst::Set(_)
                | Inst::AnyButCrlf => match self.advance(inst, at)? {
                    Some(next) => {
                        at = next;
                        pc = pc.saturating_add(1);
                    }
                    None => return Ok(false),
                },
            }
        }
    }

    /// Where `inst`, neither a fork, a jump nor the match, leaves the
    /// search at `at`: none if it fails there.
    fn advance(&mut self, inst: &Inst, at: usize) -> Result<Option<usize>> {
        let held = |ok: bool| ok.then_some(at);
        Ok(match inst {
            Inst::Save(slot) => {
                self.keep(*slot, at);
                Some(at)
            }
            Inst::LoopEnter(k) => {
                self.keep(self.program.slots.saturating_add(*k), at);
                Some(at)
            }
            Inst::LoopCheck(k) => held(self.slots.get(self.program.slots.saturating_add(*k)).copied().flatten() != Some(at)),
            Inst::LineStart(lines) => held(line_start(self.text, at, *lines)?),
            Inst::LineEnd(lines) => held(line_end(self.text, at, *lines)?),
            Inst::BackRef(group) => self.again(*group, at)?.map(|length| at.saturating_add(length)),
            Inst::Char(_) | Inst::Set(_) | Inst::AnyButCrlf => one(inst, self.text, at)?.then(|| self.text.after(at)),
            Inst::Match | Inst::Jump(_) | Inst::Split(..) => None,
        })
    }

    /// [F&O 3.1 §5.6.1.4, §5.6.2] How many characters group `group`'s
    /// capture matches at `at`, each the same or, under the flag `i`, a
    /// case variant: none if they do not; nothing captured matching the
    /// empty string. The capture and what is at `at` are read a character
    /// of each in turn, so neither is held whole, each by the positions
    /// of its own characters, as a case variant may take other octets
    /// than the character captured (RULES.md D1189). The count is of
    /// positions, those from `at` to the end of what matched.
    fn again(&self, group: usize, at: usize) -> Result<Option<usize>> {
        let Some(start) = group.checked_mul(2) else { return Ok(None) };
        let (Some(Some(from)), Some(Some(to))) = (self.slots.get(start), self.slots.get(start.saturating_add(1))) else {
            return Ok(Some(0));
        };
        let length = to.saturating_sub(*from);
        let alike = |a: char, b: char| self.caseless.map_or(a == b, |table| table.alike(a, b));
        if let (Some(captured), Some(here)) = (self.text.held(*from, *to), self.text.held(at, at.saturating_add(length))) {
            return Ok(captured.iter().zip(here).all(|(&a, &b)| alike(a, b)).then_some(length));
        }
        let (mut theirs, mut ours) = (*from, at);
        while theirs < *to {
            let (a, b) = (self.text.at(theirs)?, self.text.at(ours)?);
            if !a.zip(b).is_some_and(|(a, b)| alike(a, b)) {
                return Ok(None);
            }
            (theirs, ours) = (self.text.after(theirs), self.text.after(ours));
        }
        Ok(Some(ours.saturating_sub(at)))
    }
}
