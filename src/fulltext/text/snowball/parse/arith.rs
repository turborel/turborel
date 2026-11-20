//! \[DESIGN\] A Snowball arithmetic expression (`AE`) and an `among`, as
//! the analyser's `read_AE` and `read_among` read them (RULES.md D320).
//! `*` and `/` bind tighter than `+` and `-`, each from the left, and a
//! monadic minus binds to the operand after it alone. `lenof` and
//! `sizeof` count characters, as the cursor does, so of a literal string
//! each is its number of characters.
//!
//! An among's strings each choose the command in parentheses after them,
//! if one follows before the next `()`; a string may name a routine that
//! must succeed after it matches. The among after a `substring` is that
//! substring's.

use super::{Kind, Parser};
use crate::error::Result;
use crate::fulltext::text::snowball::ast::{Ae, Among, Arith, Command, Entry};
use crate::fulltext::text::snowball::damaged;
use crate::fulltext::text::snowball::lex::{Key, Sym, Token};

/// The operators, by their symbols, and how tightly each binds.
const OPERATORS: [(Sym, Arith, u8); 4] =
    [(Sym::Plus, Arith::Add, 1), (Sym::Minus, Arith::Subtract, 1), (Sym::Multiply, Arith::Multiply, 2), (Sym::Divide, Arith::Divide, 2)];

/// How tightly nothing binds, so a whole expression is read.
const LOOSEST: u8 = 0;

impl Parser {
    /// An arithmetic expression.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if none is next.
    pub(super) fn ae(&mut self) -> Result<Ae> {
        self.ae_binding(LOOSEST)
    }

    /// An expression of operators binding tighter than `binding`.
    fn ae_binding(&mut self, binding: u8) -> Result<Ae> {
        self.enter()?;
        let mut left = self.operand()?;
        while let Some((op, tighter)) = self.operator().filter(|(_, tighter)| *tighter > binding) {
            self.next = self.next.saturating_add(1);
            let right = self.ae_binding(tighter)?;
            left = Ae::Binary(Box::new(left), op, Box::new(right));
        }
        self.leave();
        Ok(left)
    }

    /// The operator next, not taken, and how tightly it binds.
    fn operator(&self) -> Option<(Arith, u8)> {
        let Some(Token::Sym(sym)) = self.peek() else { return None };
        OPERATORS.iter().find(|(written, _, _)| written == sym).map(|(_, op, tighter)| (*op, *tighter))
    }

    /// An operand.
    fn operand(&mut self) -> Result<Ae> {
        let line = self.line();
        match self.advance() {
            Some(Token::Sym(Sym::Minus)) => {
                self.enter()?;
                let negated = self.operand()?;
                self.leave();
                Ok(Ae::Negate(Box::new(negated)))
            }
            Some(Token::Sym(Sym::Bra)) => {
                let inner = self.ae()?;
                self.expect_sym(Sym::Ket)?;
                Ok(inner)
            }
            Some(Token::Name(name)) => Ok(Ae::Integer(self.resolve(&name, Kind::Integer)?)),
            Some(Token::Number(n)) => Ok(Ae::Number(n)),
            Some(Token::Key(key)) => self.key_operand(key),
            Some(Token::Sym(_) | Token::Text(_)) | None => Err(damaged(line, "expected an arithmetic expression")),
        }
    }

    /// The operand the key word `key` is.
    fn key_operand(&mut self, key: Key) -> Result<Ae> {
        if key == Key::Cursor {
            return Ok(Ae::Cursor);
        }
        if key == Key::Limit {
            return Ok(Ae::Limit);
        }
        if key == Key::Len || key == Key::Size {
            return Ok(Ae::Len);
        }
        if key == Key::Maxint {
            return Ok(Ae::Maxint);
        }
        if key == Key::Minint {
            return Ok(Ae::Minint);
        }
        if key != Key::Lenof && key != Key::Sizeof {
            return Err(damaged(self.line(), &format!("{key:?} is no arithmetic expression")));
        }
        match self.advance() {
            Some(Token::Text(text)) => Ok(Ae::Number(i64::try_from(text.len()).unwrap_or(i64::MAX))),
            Some(Token::Name(name)) => Ok(Ae::LenOf(self.resolve(&name, Kind::String)?)),
            Some(Token::Key(_) | Token::Number(_) | Token::Sym(_)) | None => Err(damaged(self.line(), "expected a string")),
        }
    }

    /// The place of an among made now, to be filled.
    pub(super) fn new_among(&mut self) -> usize {
        self.amongs.push(None);
        self.amongs.len().saturating_sub(1)
    }

    /// `among ( ... )`, its key word taken.
    pub(super) fn among(&mut self) -> Result<Command> {
        self.expect_sym(Sym::Bra)?;
        let (place, substring) = match self.substring.take() {
            Some(place) => (place, true),
            None => (self.new_among(), false),
        };
        let mut among = Among::default();
        // The first string no command has been given to yet.
        let mut waiting = 0_usize;
        let mut after_command = false;
        loop {
            match self.advance() {
                Some(Token::Text(text)) => {
                    self.among_string(&mut among, text)?;
                    after_command = false;
                }
                Some(Token::Sym(Sym::Bra)) => {
                    if after_command {
                        return Err(damaged(self.line(), "two commands follow each other in an among"));
                    }
                    let command = self.among_command()?;
                    give(&mut among, waiting, command);
                    waiting = among.entries.len();
                    after_command = true;
                }
                Some(Token::Sym(Sym::Ket)) => break,
                Some(Token::Key(_) | Token::Name(_) | Token::Number(_) | Token::Sym(_)) | None => {
                    return Err(damaged(self.line(), "expected a string or command of an among"));
                }
            }
        }
        if among.entries.is_empty() {
            return Err(damaged(self.line(), "an among has no string"));
        }
        let mut lengths: Vec<usize> = among.index.keys().map(Vec::len).collect();
        lengths.sort_unstable_by(|a, b| b.cmp(a));
        lengths.dedup();
        among.lengths = lengths;
        if let Some(slot) = self.amongs.get_mut(place) {
            *slot = Some(among);
        }
        Ok(Command::Among { among: place, substring })
    }

    /// A string of `among`, `text`, and the routine after it, if one is.
    fn among_string(&mut self, among: &mut Among, text: Vec<char>) -> Result<()> {
        let routine = match self.peek() {
            Some(Token::Name(name)) => {
                let name = name.clone();
                self.next = self.next.saturating_add(1);
                Some(self.resolve(&name, Kind::Routine)?)
            }
            Some(Token::Key(_) | Token::Number(_) | Token::Text(_) | Token::Sym(_)) | None => None,
        };
        if among.index.insert(text, among.entries.len()).is_some() {
            return Err(damaged(self.line(), "a string is twice in an among"));
        }
        among.entries.push(Entry { routine, action: None });
        Ok(())
    }

    /// The command of an among, its `(` taken: none for `()`.
    fn among_command(&mut self) -> Result<Option<Command>> {
        if self.accept_sym(Sym::Ket) {
            return Ok(None);
        }
        self.next = self.next.saturating_sub(1);
        self.command().map(Some)
    }
}

/// `command`, if there is one, given to the strings of `among` from the
/// place `waiting`.
fn give(among: &mut Among, waiting: usize, command: Option<Command>) {
    let Some(command) = command else { return };
    let action = among.actions.len();
    among.actions.push(command);
    for entry in among.entries.iter_mut().skip(waiting) {
        entry.action = Some(action);
    }
}
