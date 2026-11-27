//! \[DESIGN\] (RULES.md D1251) A LIKE pattern's machine
//! (`value/like/machine.rs`) stepped over texts coded by a table of
//! symbols, a code at a time, so a text is matched with no octet of it
//! decoded. Searching a text's codes for the codes of a piece would be
//! wrong, as the longest symbol is taken at each place, so one piece is
//! coded in as many ways as there are octets before it. The machine is
//! stepped instead. For each state and each symbol, the state after the
//! symbol's octets, each stepped in turn, is found once and kept in a
//! table, and a text is matched by one place of the table a code. A
//! piece that ends inside a symbol is found there, as the octets after
//! it in the symbol step the machine on from the piece's end, into the
//! states of the piece after it or into the state that accepts whatever
//! follows.
//!
//! A code of `ESCAPE` quotes the octet after it, which steps the machine
//! by itself. So a state's row has two halves of 256 places, which hold
//! the state after each code, the escape's leaving the state as it is,
//! and the state after each quoted octet. A text's codes are read one after
//! another, and all that is carried from one to the next, beside the
//! state, is whether it is quoted.
//!
//! In most of a text the machine stays in one state, the start of a
//! piece searched for, as few symbols end with a start of the piece. So
//! the codes are read in runs that leave the state as it is, a run's
//! loop asking only whether the place it reads holds the state it is in,
//! which waits for no place read before it. A step that changes the
//! state ends the run, and the next begins in the new state's row.

use super::{ESCAPE, Symbols};
use crate::value::like::machine::{Machine, OCTETS, START};

/// The places of a state's row: each code, then each quoted octet.
const ROW: usize = 2 * OCTETS;

/// \[DESIGN\] (RULES.md D1251) A pattern's machine as a table of
/// symbols steps it: the state after each state and code, and after
/// each state and quoted octet.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::engine::columns) struct Steps {
    /// Each state's row.
    after: Vec<[u8; ROW]>,
    /// The state a text that matches ends in.
    accept: u8,
    /// The first of the states no octet leaves.
    settled: u8,
    /// The state after a code of no symbol, past the machine's own.
    malformed: u8,
}

impl Symbols {
    /// The state after `state` of `machine` reads the octets of the
    /// symbol of `code`.
    fn stepped(&self, machine: &Machine, state: u8, code: usize) -> Option<u8> {
        let octets = self.words.get(code)?.get(..usize::from(*self.lens.get(code)?))?;
        Some(octets.iter().fold(state, |at, &octet| machine.step(at, octet)))
    }

    /// \[DESIGN\] (RULES.md D1251) `machine` as this table steps it:
    /// made once a pattern and a table, of two octets a state for each
    /// value of an octet, 130,560 octets at most (`MOST_STATES`).
    pub(in crate::engine::columns) fn steps(&self, machine: &Machine) -> Steps {
        // A machine has 255 states at most, so the value past them is an
        // octet's too.
        let malformed = u8::try_from(machine.states()).unwrap_or(u8::MAX);
        let mut after = vec![[malformed; ROW]; machine.states()];
        for (state, row) in (0..=u8::MAX).zip(after.iter_mut()) {
            let (codes, quoted) = row.split_at_mut(OCTETS);
            for (code, place) in codes.iter_mut().enumerate().take(self.count) {
                *place = self.stepped(machine, state, code).unwrap_or(malformed);
            }
            // The escape steps nothing, as the octet it quotes does.
            if let Some(place) = codes.get_mut(usize::from(ESCAPE)) {
                *place = state;
            }
            for (octet, place) in (0..=u8::MAX).zip(quoted.iter_mut()) {
                *place = machine.step(state, octet);
            }
        }
        Steps { after, accept: machine.accept(), settled: machine.first_settled(), malformed }
    }
}

impl Steps {
    /// [X3.135-1986 §5.14 GR5] Whether the text `codes` encode matches
    /// the pattern: none if a code is of no symbol, or they end in an
    /// escape, before the text is known to match or not.
    pub(in crate::engine::columns) fn matches(&self, codes: &[u8]) -> Option<bool> {
        let (mut state, mut quoted) = (START, false);
        let mut rest = codes.iter();
        'text: while state < self.settled {
            let row = self.after.get(usize::from(state))?;
            // A run of codes that leave the state as it is.
            loop {
                let Some(&octet) = rest.next() else { break 'text };
                let half = if quoted { OCTETS } else { 0 };
                let next = row.get(half | usize::from(octet)).copied()?;
                quoted = !quoted && octet == ESCAPE;
                if next != state {
                    state = next;
                    break;
                }
            }
        }
        // Read to their end, they may not end in an escape.
        if state == self.malformed || (state < self.settled && quoted) {
            return None;
        }
        Some(state == self.accept)
    }
}

/// \[DESIGN\] (RULES.md D1251) The tables of steps of a scan's LIKEs,
/// each kept with the table of symbols it was made of. A group's chunks
/// of a column are coded by one table, written in each, so a LIKE's
/// steps are made once a group and found again by its chunks' tables
/// being the same.
#[derive(Debug, Default)]
pub(in crate::engine::columns) struct Stepped {
    /// Of each LIKE, by its conjunct's place in the scan's condition,
    /// the table of symbols last read and its steps.
    held: Vec<(usize, Box<Symbols>, Steps)>,
    /// How many tables of steps were made, as a test counts them.
    #[cfg(test)]
    pub(in crate::engine::columns) made: usize,
}

impl Stepped {
    /// The steps of `machine`, the LIKE at place `at` of a scan's
    /// condition, by `symbols`: those held, if made of the same table,
    /// else made and held in their place.
    pub(in crate::engine::columns) fn of(&mut self, at: usize, symbols: &Symbols, machine: &Machine) -> Option<&Steps> {
        let found = self.held.iter().position(|(place, ..)| *place == at);
        let same = found.and_then(|n| self.held.get(n)).is_some_and(|(_, of, _)| **of == *symbols);
        if !same {
            #[cfg(test)]
            {
                self.made = self.made.saturating_add(1);
            }
            let made = (at, Box::new(*symbols), symbols.steps(machine));
            match found.and_then(|n| self.held.get_mut(n)) {
                Some(slot) => *slot = made,
                None => self.held.push(made),
            }
        }
        let n = found.unwrap_or(self.held.len().saturating_sub(1));
        self.held.get(n).map(|(_, _, steps)| steps)
    }
}

// Unit tests: the steps against the pattern's own matching of the text
// decoded (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
