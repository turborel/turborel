//! \[DESIGN\] (RULES.md D1122) A pattern of no `_` matched of a value's
//! codes, as a table of symbols codes it, no text decoded.
//!
//! A table of symbols writes each value as codes, each code one symbol of
//! one to eight bytes, or an escape followed by one byte no symbol takes.
//! The same piece of a pattern is written by different codes as the bytes
//! before it fall, so a piece's own codes cannot be looked for among a
//! value's. The pattern is made instead into a machine over bytes, and
//! each code's step is that of its symbol's bytes one after another, made
//! once for the table.
//!
//! The machine takes the pieces in order, as `like/pieces.rs` matches
//! them. First the value's first bytes must be the first piece. Then each
//! piece between is looked for from where the one before it ended, by its
//! prefix function (Knuth, Morris and Pratt, "Fast Pattern Matching in
//! Strings", SIAM J. Comput. 1977), and the first place it ends is taken.
//! Then the bytes after the last piece between must end with the last
//! piece. A value matches if the machine ends there. Taking each piece
//! between at the first place it ends leaves the most bytes to the pieces
//! after it, so no other placement matches where this one fails, as D1074
//! argues. A piece is whole characters, so bytes match where characters
//! do. A piece that spans two symbols, or an escape and a symbol, is
//! stepped through byte by byte all the same, as the machine never looks
//! at where a symbol ends.
//!
//! An empty piece between is none, as `Pieces` keeps none. An empty last
//! piece, of a pattern that ends in `%`, is met once the pieces between
//! are, and the rest of the value is then not read. A pattern of no `%`
//! is the first piece alone, which the value must be exactly.

use std::collections::HashMap;

use super::LikePattern;
use super::pieces::Pieces;

/// The code an escaped byte follows, as the tables of symbols write it.
pub const ESCAPE: u8 = 255;

/// \[DESIGN\] A pattern's machine made for one table of symbols.
#[derive(Debug, Clone)]
pub struct CodeMatcher {
    /// Of each state, the state after each byte.
    by_byte: Vec<u16>,
    /// Of each state, the state after each code's symbol.
    by_code: Vec<u16>,
    /// The codes the table has symbols for.
    codes: usize,
    /// The state a value starts in.
    start: u16,
    /// Of each state: 0 to go on, 1 if a value ending there matches, 2 if
    /// any value reaching it matches, 3 if none does.
    ends: Vec<u8>,
}

/// A state of the machine, before it is numbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum State {
    /// That many bytes of the first piece read.
    Prefix(usize),
    /// The first piece read, of a pattern of no `%`.
    Exact,
    /// Of the `i`th piece between, the bytes of it last read.
    Between(usize, usize),
    /// Of the last piece, the bytes of it the bytes read end with.
    Suffix(usize),
    /// Every piece met, of a pattern ending in `%`.
    Met,
    /// No value reaching it matches.
    Dead,
}

/// What steps a byte through the pieces.
struct Machine<'a> {
    first: &'a [u8],
    between: Vec<(&'a [u8], Vec<usize>)>,
    last: Option<(&'a [u8], Vec<usize>)>,
}

/// The prefix function of `piece`: of each length, the longest proper
/// prefix of that many bytes that is also a suffix of them.
fn failures(piece: &[u8]) -> Vec<usize> {
    let mut fail = vec![0_usize; piece.len()];
    let mut k = 0_usize;
    for i in 1..piece.len() {
        while k > 0 && piece.get(i) != piece.get(k) {
            k = fail.get(k.saturating_sub(1)).copied().unwrap_or(0);
        }
        if piece.get(i) == piece.get(k) {
            k = k.saturating_add(1);
        }
        if let Some(slot) = fail.get_mut(i) {
            *slot = k;
        }
    }
    fail
}

/// Of a piece of prefix function `fail`, `j` of its bytes matched, the
/// bytes matched after `b`.
fn advanced((piece, fail): (&[u8], &[usize]), mut j: usize, b: u8) -> usize {
    while j > 0 && (j >= piece.len() || piece.get(j) != Some(&b)) {
        j = fail.get(j.saturating_sub(1)).copied().unwrap_or(0);
    }
    if piece.get(j) == Some(&b) { j.saturating_add(1) } else { j }
}

impl Machine<'_> {
    /// `state` moved past each piece it has read whole.
    fn settled(&self, state: State) -> State {
        match state {
            State::Prefix(j) if j >= self.first.len() => {
                if self.last.is_none() {
                    State::Exact
                } else {
                    self.settled(State::Between(0, 0))
                }
            }
            State::Between(i, _) if i >= self.between.len() => match &self.last {
                Some((last, _)) if !last.is_empty() => State::Suffix(0),
                Some(_) | None => State::Met,
            },
            State::Prefix(_) | State::Exact | State::Between(..) | State::Suffix(_) | State::Met | State::Dead => state,
        }
    }

    /// The state after `b` from `state`.
    fn step(&self, state: State, b: u8) -> State {
        match state {
            State::Prefix(j) if self.first.get(j) == Some(&b) => self.settled(State::Prefix(j.saturating_add(1))),
            State::Prefix(_) | State::Exact | State::Dead => State::Dead,
            State::Between(i, j) => {
                let Some((piece, fail)) = self.between.get(i) else { return State::Dead };
                let next = advanced((piece, fail), j, b);
                if next >= piece.len() { self.settled(State::Between(i.saturating_add(1), 0)) } else { State::Between(i, next) }
            }
            State::Suffix(j) => self.last.as_ref().map_or(State::Dead, |(last, fail)| State::Suffix(advanced((last, fail), j, b))),
            State::Met => State::Met,
        }
    }

    /// What a value ending in `state` is: as `CodeMatcher::ends` has it.
    fn end(&self, state: State) -> u8 {
        match state {
            State::Exact => 1,
            State::Suffix(j) if self.last.as_ref().is_some_and(|(last, _)| j == last.len()) => 1,
            State::Met => 2,
            State::Dead => 3,
            State::Prefix(_) | State::Between(..) | State::Suffix(_) => 0,
        }
    }

    /// Every state, numbered as `CodeMatcher` keeps them.
    fn states(&self) -> Vec<State> {
        let mut all: Vec<State> = (0..self.first.len()).map(State::Prefix).collect();
        all.push(State::Exact);
        for (i, (piece, _)) in self.between.iter().enumerate() {
            all.extend((0..piece.len()).map(|j| State::Between(i, j)));
        }
        if let Some((last, _)) = &self.last {
            all.extend((0..=last.len()).map(State::Suffix));
        }
        all.push(State::Met);
        all.push(State::Dead);
        all
    }
}

impl CodeMatcher {
    /// The machine of `pieces` for a table of `symbols`, each code's bytes
    /// by its place; none if it has more states than a table holds.
    fn of(pieces: &Pieces, symbols: &[&[u8]]) -> Option<CodeMatcher> {
        let machine = Machine {
            first: pieces.first.as_bytes(),
            between: pieces.between.iter().map(|f| (f.bytes(), failures(f.bytes()))).collect(),
            last: pieces.last.as_ref().map(|l| (l.as_bytes(), failures(l.as_bytes()))),
        };
        let states = machine.states();
        if states.len() > usize::from(u16::MAX) {
            return None;
        }
        let places: HashMap<State, u16> = states.iter().zip(0_u16..).map(|(&s, n)| (s, n)).collect();
        let number = |s: State| places.get(&s).copied().unwrap_or(0);
        let mut by_byte = Vec::with_capacity(states.len().saturating_mul(256));
        let mut by_code = Vec::with_capacity(states.len().saturating_mul(symbols.len()));
        for &state in &states {
            by_byte.extend((0..=u8::MAX).map(|b| number(machine.step(state, b))));
            by_code.extend(symbols.iter().map(|bytes| number(bytes.iter().fold(state, |s, &b| machine.step(s, b)))));
        }
        let ends = states.iter().map(|&s| machine.end(s)).collect();
        Some(CodeMatcher { by_byte, by_code, codes: symbols.len(), start: number(machine.settled(State::Prefix(0))), ends })
    }

    /// [X3.135-1986 §5.14 GR5] Whether the value `codes` write matches the
    /// pattern; none if they end in an escape or a code is of no symbol,
    /// which the value's decoding then reports.
    pub fn matches(&self, codes: &[u8]) -> Option<bool> {
        let mut state = usize::from(self.start);
        let mut rest = codes.iter();
        while let Some(&c) = rest.next() {
            match self.ends.get(state) {
                Some(2) => return Some(true),
                Some(3) => return Some(false),
                _ => {}
            }
            let next = if c == ESCAPE {
                let b = *rest.next()?;
                self.by_byte.get(state.saturating_mul(256).saturating_add(usize::from(b)))
            } else {
                let code = usize::from(c);
                if code >= self.codes {
                    return None;
                }
                self.by_code.get(state.saturating_mul(self.codes).saturating_add(code))
            };
            state = usize::from(*next?);
        }
        Some(matches!(self.ends.get(state), Some(1 | 2)))
    }
}

impl LikePattern {
    /// Its machine for a table of `symbols`, each code's bytes by its
    /// place (RULES.md D1122): none of a pattern with `_`, which is
    /// matched of the decoded values.
    pub fn on_codes(&self, symbols: &[&[u8]]) -> Option<CodeMatcher> {
        CodeMatcher::of(self.pieces.as_ref()?, symbols)
    }
}

// Unit tests: the machine against the pieces' own matching of the decoded
// values (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
