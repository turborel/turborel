//! \[DESIGN\] The matcher of a program with no back-reference: threads run
//! over the string in lockstep, each instruction at most once at each
//! position, so a match is found in time at most the product of the
//! string's length and the program's, whatever the pattern (RULES.md D61).
//! Threads are kept in the order a backtracking matcher would try them, so
//! the match found is the one F&O 3.1 §5.6 gives: the leftmost, and of
//! those the one alternation's order and the quantifiers' greed prefer.

use super::compile::{Inst, Program};
use super::text::{Text, line_end, line_start, one};
use crate::error::Result;

/// Where each group starts and ends, if it has.
pub(super) type Slots = Vec<Option<usize>>;

/// A thread: where it is in the program, and what it has captured.
pub(super) struct Thread {
    pc: usize,
    slots: Slots,
}

/// The threads at one position, in the order they are preferred, and
/// which instructions they have reached.
struct List {
    threads: Vec<Thread>,
    seen: Vec<bool>,
}

impl List {
    fn new(instructions: usize) -> List {
        List { threads: Vec::new(), seen: vec![false; instructions] }
    }
}

/// Work left in following a thread's jumps and forks.
enum Job {
    /// Follow the instruction.
    Visit(usize),
    /// Put a slot back as it was.
    Restore(usize, Option<usize>),
}

/// What a search reads and keeps.
struct Search<'a, T: Text + ?Sized> {
    program: &'a Program,
    text: &'a T,
    stack: Vec<Job>,
}

/// The first match of `program` in `text` beginning at `from` or after,
/// each slot of where its groups are if `capture`, else none of them. It
/// reads `text` forward, each character once but those an assertion
/// reads beside it.
///
/// # Errors
///
/// As reading `text` fails.
pub(super) fn search<T: Text + ?Sized>(program: &Program, text: &T, from: usize, capture: bool) -> Result<Option<Slots>> {
    let instructions = program.insts.len();
    let mut s = Search { program, text, stack: Vec::new() };
    let (mut current, mut next) = (List::new(instructions), List::new(instructions));
    let mut slots: Slots = vec![None; if capture { program.slots } else { 0 }];
    let mut matched = None;
    let mut at = from;
    loop {
        if matched.is_none() {
            slots.fill(None);
            s.add(&mut current, 0, at, &mut slots)?;
        }
        if current.threads.is_empty() && (matched.is_some() || at >= text.len()) {
            break;
        }
        for thread in current.threads.drain(..) {
            let Some(inst) = program.insts.get(thread.pc) else { continue };
            if *inst == Inst::Match {
                // Each thread after this one is less preferred.
                matched = Some(thread.slots);
                if !capture {
                    return Ok(matched);
                }
                break;
            }
            if one(inst, text, at)? {
                let mut kept = thread.slots;
                s.add(&mut next, thread.pc.saturating_add(1), at.saturating_add(1), &mut kept)?;
            }
        }
        if at >= text.len() {
            break;
        }
        std::mem::swap(&mut current, &mut next);
        next.threads.clear();
        next.seen.fill(false);
        at = at.saturating_add(1);
    }
    Ok(matched)
}

impl<T: Text + ?Sized> Search<'_, T> {
    /// Adds to `list` the threads `from` leads to at `at`, following jumps,
    /// forks, saves and assertions, each in the order it is preferred.
    fn add(&mut self, list: &mut List, from: usize, at: usize, slots: &mut Slots) -> Result<()> {
        self.stack.push(Job::Visit(from));
        while let Some(job) = self.stack.pop() {
            let pc = match job {
                Job::Restore(slot, old) => {
                    if let Some(s) = slots.get_mut(slot) {
                        *s = old;
                    }
                    continue;
                }
                Job::Visit(pc) => pc,
            };
            let Some(seen) = list.seen.get_mut(pc) else { continue };
            if *seen {
                continue;
            }
            *seen = true;
            let Some(inst) = self.program.insts.get(pc) else { continue };
            let after = pc.saturating_add(1);
            match inst {
                Inst::Jump(to) => self.stack.push(Job::Visit(*to)),
                Inst::Split(first, second) => {
                    self.stack.push(Job::Visit(*second));
                    self.stack.push(Job::Visit(*first));
                }
                Inst::Save(slot) => {
                    if let Some(s) = slots.get_mut(*slot) {
                        self.stack.push(Job::Restore(*slot, *s));
                        *s = Some(at);
                    }
                    self.stack.push(Job::Visit(after));
                }
                Inst::LineStart(lines) => self.assert(line_start(self.text, at, *lines)?, after),
                Inst::LineEnd(lines) => self.assert(line_end(self.text, at, *lines)?, after),
                // An iteration that matches nothing reaches an instruction
                // already seen at this position, so needs no check here.
                Inst::LoopEnter(_) | Inst::LoopCheck(_) => self.stack.push(Job::Visit(after)),
                Inst::Char(_) | Inst::Set(_) | Inst::AnyButCrlf | Inst::BackRef(_) | Inst::Match => {
                    list.threads.push(Thread { pc, slots: slots.clone() });
                }
            }
        }
        Ok(())
    }

    /// Goes on at `after` if `holds`.
    fn assert(&mut self, holds: bool, after: usize) {
        if holds {
            self.stack.push(Job::Visit(after));
        }
    }
}
