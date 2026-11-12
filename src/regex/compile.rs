//! A regular expression read, compiled to a program both matchers run:
//! instructions that match a character, assert a position, fork, jump,
//! save where a group starts or ends, or match a back-reference; and the
//! line boundaries of UTS #18 RL1.6 that `^`, `$` and `.` keep
//! [ISO/IEC 9075-2:2023 §9.28 GR9a i].

use super::class::{CharSet, NEWLINES, fixed_escape, not_newline};
use super::flags::{Flag, Flags};
use super::parse::{MAX_NESTING, Node, Parsed, Quantity};
use crate::error::Result;

/// [DESIGN] The most instructions a program has, so matching takes time at
/// most this many times the length of the string (CONFORMANCE.md).
pub(super) const MAX_INSTRUCTIONS: usize = 8192;
/// How deep the tree read is at most: each group nests a branch, a piece
/// and an atom within it.
const MAX_TREE_DEPTH: usize = 4 * (MAX_NESTING + 1);
/// The slots of a group: where it starts and where it ends.
const SLOTS_PER_GROUP: usize = 2;

/// One instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Inst {
    /// The character.
    Char(char),
    /// A character of the set.
    Set(CharSet),
    /// [§9.28 GR9a i; UTS #18 RL1.6] Under the flag `s`, any character
    /// but a CR before an LF, which the CRLF beside it matches.
    AnyButCrlf,
    /// `^` [F&O 3.1 §5.6.1.1], of each line if multi-line.
    LineStart(bool),
    /// `$`, of each line if multi-line.
    LineEnd(bool),
    /// Go on at both, the first preferred.
    Split(usize, usize),
    /// Go on there.
    Jump(usize),
    /// Keep the position in the slot.
    Save(usize),
    /// What the group last captured, again [F&O 3.1 §5.6.1.4].
    BackRef(usize),
    /// Keep the position in the loop's slot, as an iteration begins.
    LoopEnter(usize),
    /// Fail unless the iteration moved past the position kept, so an
    /// iteration that matches nothing is not taken again and again.
    LoopCheck(usize),
    /// A match.
    Match,
}

/// A program, and what running it needs.
#[derive(Debug, Clone)]
pub(super) struct Program {
    /// Its instructions, the first first.
    pub(super) insts: Vec<Inst>,
    /// How many slots its groups have, the whole match's first.
    pub(super) slots: usize,
    /// How many loops keep a position.
    pub(super) loops: usize,
}

/// Compiles `parsed` under `flags`, `.` matching newlines under `s`, `^`
/// and `$` at each line under `m`.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_INSTRUCTIONS`.
pub(super) fn compile(parsed: &Parsed, flags: Flags) -> Result<Program> {
    let (dot_all, multiline) = (flags.has(Flag::DotAll), flags.has(Flag::MultiLine));
    let mut c = Compiler { insts: Vec::new(), loops: 0, dot_all, multiline };
    c.emit(Inst::Save(0))?;
    c.node(&parsed.node, 0)?;
    c.emit(Inst::Save(1))?;
    c.emit(Inst::Match)?;
    let slots = parsed.groups.saturating_add(1).saturating_mul(SLOTS_PER_GROUP);
    Ok(Program { insts: c.insts, slots, loops: c.loops })
}

struct Compiler {
    insts: Vec<Inst>,
    loops: usize,
    dot_all: bool,
    multiline: bool,
}

impl Compiler {
    /// Adds `inst`: where it is.
    fn emit(&mut self, inst: Inst) -> Result<usize> {
        if self.insts.len() >= MAX_INSTRUCTIONS {
            return Err(super::beyond(&format!("the pattern compiles to more than {MAX_INSTRUCTIONS} instructions")));
        }
        self.insts.push(inst);
        Ok(self.insts.len().saturating_sub(1))
    }

    /// Puts `inst` at `at`, which `emit` gave.
    fn patch(&mut self, at: usize, inst: Inst) {
        if let Some(slot) = self.insts.get_mut(at) {
            *slot = inst;
        }
    }

    /// Where the next instruction goes.
    fn here(&self) -> usize {
        self.insts.len()
    }

    fn node(&mut self, n: &Node, depth: usize) -> Result<()> {
        if depth > MAX_TREE_DEPTH {
            return Err(super::beyond(&format!("groups nest more than {MAX_NESTING} deep")));
        }
        let deeper = depth.saturating_add(1);
        match n {
            Node::Empty => {}
            Node::Set(set) => self.set(set)?,
            Node::Dot => self.dot()?,
            Node::Space => self.crlf_or(Inst::Set(fixed_escape('s').unwrap_or_default()))?,
            Node::Start => {
                self.emit(Inst::LineStart(self.multiline))?;
            }
            Node::End => {
                self.emit(Inst::LineEnd(self.multiline))?;
            }
            Node::Group(inner, number) => self.group(inner, *number, deeper)?,
            Node::Concat(pieces) => {
                for piece in pieces {
                    self.node(piece, deeper)?;
                }
            }
            Node::Alt(branches) => self.alternation(branches, deeper)?,
            Node::Repeat(inner, q) => self.repeat(inner, *q, deeper)?,
            Node::BackRef(group) => {
                self.emit(Inst::BackRef(*group))?;
            }
        }
        Ok(())
    }

    /// [§9.28 GR9a i] The wildcard: under the flag `s` any character,
    /// CRLF one; else any but a newline's.
    fn dot(&mut self) -> Result<()> {
        if self.dot_all { self.crlf_or(Inst::AnyButCrlf) } else { self.set(&not_newline()) }
    }

    /// One character of `set`, a `Char` if it holds one alone.
    fn set(&mut self, set: &CharSet) -> Result<()> {
        let one = if let [(first, last)] = set.ranges() { char::from_u32(*first).filter(|_| first == last) } else { None };
        self.emit(one.map_or_else(|| Inst::Set(set.clone()), Inst::Char))?;
        Ok(())
    }

    /// CRLF, preferred, or else `single` [§9.28 GR9a ii; UTS #18 RL1.6].
    fn crlf_or(&mut self, single: Inst) -> Result<()> {
        let split = self.emit(Inst::Split(0, 0))?;
        self.emit(Inst::Char('\r'))?;
        self.emit(Inst::Char('\n'))?;
        let jump = self.emit(Inst::Jump(0))?;
        let other = self.emit(single)?;
        self.patch(split, Inst::Split(split.saturating_add(1), other));
        self.patch(jump, Inst::Jump(self.here()));
        Ok(())
    }

    fn group(&mut self, inner: &Node, number: Option<usize>, depth: usize) -> Result<()> {
        let slot = number.map(|g| g.saturating_mul(SLOTS_PER_GROUP));
        if let Some(s) = slot {
            self.emit(Inst::Save(s))?;
        }
        self.node(inner, depth)?;
        if let Some(s) = slot {
            self.emit(Inst::Save(s.saturating_add(1)))?;
        }
        Ok(())
    }

    /// Branches, each tried before those after it.
    fn alternation(&mut self, branches: &[Node], depth: usize) -> Result<()> {
        let mut jumps = Vec::new();
        let last = branches.len().saturating_sub(1);
        for (i, branch) in branches.iter().enumerate() {
            if i == last {
                self.node(branch, depth)?;
                break;
            }
            let split = self.emit(Inst::Split(0, 0))?;
            self.node(branch, depth)?;
            jumps.push(self.emit(Inst::Jump(0))?);
            self.patch(split, Inst::Split(split.saturating_add(1), self.here()));
        }
        let end = self.here();
        for jump in jumps {
            self.patch(jump, Inst::Jump(end));
        }
        Ok(())
    }

    /// `inner` repeated as `q` says: its least copies, then copies each
    /// optional only if the one before was taken, or a loop.
    fn repeat(&mut self, inner: &Node, q: Quantity, depth: usize) -> Result<()> {
        for _ in 0..q.min {
            self.node(inner, depth)?;
        }
        let Some(max) = q.max else { return self.star(inner, q.greedy, depth) };
        let mut splits = Vec::new();
        for _ in q.min..max {
            splits.push(self.emit(Inst::Split(0, 0))?);
            self.node(inner, depth)?;
        }
        let end = self.here();
        for split in splits {
            let body = split.saturating_add(1);
            self.patch(split, if q.greedy { Inst::Split(body, end) } else { Inst::Split(end, body) });
        }
        Ok(())
    }

    /// `inner` taken any number of times, as many as can be if `greedy`.
    fn star(&mut self, inner: &Node, greedy: bool, depth: usize) -> Result<()> {
        let loop_slot = self.loops;
        self.loops = self.loops.saturating_add(1);
        let split = self.emit(Inst::Split(0, 0))?;
        self.emit(Inst::LoopEnter(loop_slot))?;
        self.node(inner, depth)?;
        self.emit(Inst::LoopCheck(loop_slot))?;
        self.emit(Inst::Jump(split))?;
        let (body, end) = (split.saturating_add(1), self.here());
        self.patch(split, if greedy { Inst::Split(body, end) } else { Inst::Split(end, body) });
        Ok(())
    }
}

/// The character at `at` of `text`, if there is one.
pub(super) fn char_at(text: &[char], at: usize) -> Option<char> {
    text.get(at).copied()
}

/// Whether `at` is between a CR and an LF, no line boundary [UTS #18
/// RL1.6].
fn within_crlf(text: &[char], at: usize) -> bool {
    at.checked_sub(1).and_then(|before| char_at(text, before)) == Some('\r') && char_at(text, at) == Some('\n')
}

/// [F&O 3.1 §5.6.1.1, §5.6.2; §9.28 GR9a i] Whether `^` matches at `at`:
/// the string's start; and multi-line, after a newline sequence but at the
/// string's end.
pub(super) fn line_start(text: &[char], at: usize, multiline: bool) -> bool {
    if at == 0 {
        return true;
    }
    let after_newline = at.checked_sub(1).and_then(|before| char_at(text, before)).is_some_and(|c| NEWLINES.contains(&c));
    multiline && after_newline && at < text.len() && !within_crlf(text, at)
}

/// [F&O 3.1 §5.6.1.1, §5.6.2; §9.28 GR9a i] Whether `$` matches at `at`:
/// the string's end; and multi-line, before a newline sequence, the
/// string's end only if no newline ends it.
pub(super) fn line_end(text: &[char], at: usize, multiline: bool) -> bool {
    if !multiline {
        return at == text.len();
    }
    let ends_in_newline = text.last().is_some_and(|c| NEWLINES.contains(c));
    let before_newline = char_at(text, at).is_some_and(|c| NEWLINES.contains(&c)) && !within_crlf(text, at);
    before_newline || (at == text.len() && !ends_in_newline)
}

/// Whether `inst`, one that matches a character, matches the one at `at`.
pub(super) fn one(inst: &Inst, text: &[char], at: usize) -> bool {
    let Some(c) = char_at(text, at) else { return false };
    match inst {
        Inst::Char(want) => c == *want,
        Inst::Set(set) => set.contains(c),
        Inst::AnyButCrlf => !(c == '\r' && char_at(text, at.saturating_add(1)) == Some('\n')),
        Inst::LineStart(_)
        | Inst::LineEnd(_)
        | Inst::Split(..)
        | Inst::Jump(_)
        | Inst::Save(_)
        | Inst::BackRef(_)
        | Inst::LoopEnter(_)
        | Inst::LoopCheck(_)
        | Inst::Match => false,
    }
}
