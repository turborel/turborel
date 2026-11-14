//! [ISO/IEC 9075-2:2023 §9.41 GR7 to GR10] The search for the preferred
//! match beginning at a row. It follows a program's instructions, keeping
//! each fork's less preferred way on a stack of its own, not the call
//! stack, and going back to the latest on failing. The program lists the
//! preferred way first at each fork, so the strings of the parenthesized
//! language are tried in the order of preferment and the first candidate
//! met is the preferred match [GR10].

use super::compile::{Code, Instr};
use super::{MAX_STEPS, Mapped, beyond};
use crate::error::Result;

/// A quantifier's counter: the iterations begun, and the position the
/// latest began at.
#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    count: u64,
    began: usize,
}

/// Work kept for going back.
#[derive(Debug, Clone, Copy)]
enum Job {
    /// A less preferred way, at `pc` with `mapped` rows mapped.
    Try { pc: usize, mapped: usize },
    /// A counter's value to put back on going back past where it changed.
    Restore { slot: usize, old: Slot },
}

/// What one instruction leaves the search to do.
enum Next {
    /// Goes on at the instruction given.
    Go(usize),
    /// This way fails.
    Fail,
}

/// What a search reads and keeps.
pub(super) struct Search<'a> {
    code: &'a Code,
    first: usize,
    start: usize,
    end: usize,
    path: Vec<Mapped>,
    slots: Vec<Slot>,
    jobs: Vec<Job>,
    steps: u64,
}

impl<'a> Search<'a> {
    /// A search of `code` for the match of the rows `first` to `end`,
    /// excluded, that begins at row `start`.
    pub(super) fn new(code: &'a Code, (first, start, end): (usize, usize, usize)) -> Search<'a> {
        Search { code, first, start, end, path: Vec::new(), slots: vec![Slot::default(); code.slots], jobs: Vec::new(), steps: 0 }
    }

    /// [§9.41 GR10] The rows of the preferred match, none if there is no
    /// candidate.
    ///
    /// # Errors
    ///
    /// `holds`'s; \[DESIGN\] SQLCODE -900 past `MAX_STEPS`.
    pub(super) fn run<H>(mut self, holds: &mut H) -> Result<Option<Vec<Mapped>>>
    where
        H: FnMut(usize, &[Mapped]) -> Result<bool>,
    {
        let mut pc = 0;
        loop {
            let code = self.code;
            let next = match code.instrs.get(pc) {
                Some(instr) => self.execute(instr, pc, holds)?,
                None => return Ok(Some(self.path)),
            };
            pc = match next {
                Next::Go(to) => to,
                Next::Fail => match self.back() {
                    Some(to) => to,
                    None => return Ok(None),
                },
            };
        }
    }

    /// Goes back to the latest way kept, putting its counters back as they
    /// were there, and gives where that way begins, none if no way is left.
    fn back(&mut self) -> Option<usize> {
        while let Some(job) = self.jobs.pop() {
            match job {
                Job::Try { pc, mapped } => {
                    self.path.truncate(mapped);
                    return Some(pc);
                }
                Job::Restore { slot, old } => {
                    if let Some(s) = self.slots.get_mut(slot) {
                        *s = old;
                    }
                }
            }
        }
        None
    }

    /// Counts a step.
    ///
    /// # Errors
    ///
    /// \[DESIGN\] SQLCODE -900 past `MAX_STEPS`.
    fn step(&mut self) -> Result<()> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > MAX_STEPS {
            return Err(beyond(&format!("the row pattern search took more than {MAX_STEPS} steps")));
        }
        Ok(())
    }

    /// Where the next row to map is.
    fn position(&self) -> usize {
        self.start.saturating_add(self.path.len())
    }

    /// Keeps `pc` as a less preferred way from here.
    fn keep(&mut self, pc: usize) {
        self.jobs.push(Job::Try { pc, mapped: self.path.len() });
    }

    /// Sets counter `slot` to `value`, keeping the old to put back.
    fn set(&mut self, slot: usize, value: Slot) {
        if let Some(s) = self.slots.get_mut(slot) {
            self.jobs.push(Job::Restore { slot, old: *s });
            *s = value;
        }
    }

    /// Counter `slot`.
    fn slot(&self, slot: usize) -> Slot {
        self.slots.get(slot).copied().unwrap_or_default()
    }

    /// Follows `instr`, at `pc`.
    fn execute<H>(&mut self, instr: &Instr, pc: usize, holds: &mut H) -> Result<Next>
    where
        H: FnMut(usize, &[Mapped]) -> Result<bool>,
    {
        let after = pc.saturating_add(1);
        let held = |ok: bool| if ok { Next::Go(after) } else { Next::Fail };
        Ok(match instr {
            Instr::Map { variable, excluded } => held(self.map(Mapped { variable: *variable, excluded: *excluded }, holds)?),
            Instr::Start => held(self.position() == self.first),
            Instr::End => held(self.position() == self.end),
            Instr::Fail => Next::Fail,
            Instr::Fork { later } => {
                self.step()?;
                self.keep(*later);
                Next::Go(after)
            }
            Instr::Jump(to) => Next::Go(*to),
            Instr::RepeatInit { slot } => {
                self.set(*slot, Slot { count: 0, began: self.position() });
                Next::Go(after)
            }
            Instr::RepeatLoop { slot, min, max, reluctant, exit } => {
                self.step()?;
                self.choose(Choice { slot: *slot, min: *min, max: *max, more: after, exit: *exit }, *reluctant)
            }
            Instr::RepeatIter { slot } => {
                self.step()?;
                let count = self.slot(*slot).count.saturating_add(1);
                self.set(*slot, Slot { count, began: self.position() });
                Next::Go(after)
            }
            Instr::RepeatNext { slot, min, again, exit } => {
                let Slot { count, began } = self.slot(*slot);
                let empty = began == self.position();
                Next::Go(if empty && count >= *min { *exit } else { *again })
            }
        })
    }

    /// [§9.41 GR8, GR9a] Maps the next row to `mapped`'s variable, if
    /// there is a next row and `holds` judges it may be, giving whether it is.
    fn map<H>(&mut self, mapped: Mapped, holds: &mut H) -> Result<bool>
    where
        H: FnMut(usize, &[Mapped]) -> Result<bool>,
    {
        if self.position() >= self.end {
            return Ok(false);
        }
        self.step()?;
        self.path.push(mapped);
        holds(mapped.variable, &self.path)
    }

    /// [§9.41 GR6b iii, GR6c iii] Between a quantifier's iterations, the
    /// choice of another, preferred if greedy, or none, preferred if
    /// `reluctant`, where the bounds allow both. Fewer than `min` done, the
    /// bounds allow another, as `max` is never below `min` [§7.9 SR20], and
    /// one below is read as `min` (`Pattern::Repeat`).
    fn choose(&mut self, choice: Choice, reluctant: bool) -> Next {
        let count = self.slot(choice.slot).count;
        if count < choice.min {
            return Next::Go(choice.more);
        }
        if choice.max.is_some_and(|max| count >= max) {
            return Next::Go(choice.exit);
        }
        let (preferred, later) = if reluctant { (choice.exit, choice.more) } else { (choice.more, choice.exit) };
        self.keep(later);
        Next::Go(preferred)
    }
}

/// A choice between a quantifier's iterations.
#[derive(Debug, Clone, Copy)]
struct Choice {
    /// Its counter.
    slot: usize,
    /// The least number of iterations.
    min: u64,
    /// The most, none if unbounded.
    max: Option<u64>,
    /// Where another iteration begins.
    more: usize,
    /// Where the quantified pattern ends.
    exit: usize,
}
