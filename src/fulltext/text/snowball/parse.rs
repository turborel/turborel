//! \[DESIGN\] Parsing a Snowball program (RULES.md D320): its
//! declarations, routine and grouping definitions and `backwardmode`
//! blocks, as the Snowball compiler's analyser reads them (snowballstem.org,
//! `compiler/analyser.c`). Commands are read in `command.rs`, arithmetic
//! and amongs in `arith.rs`.
//!
//! Every name is declared before it is used, so each is resolved to its
//! place as it is read. A grouping is made from those defined before it.
//! Nesting is bounded by `MAX_NESTING`, so a damaged program cannot
//! exhaust the stack.

mod arith;
mod command;

use std::collections::HashMap;

use super::ast::{Among, Program, Routine};
use super::damaged;
use super::lex::{Key, Lexed, Sym, Token};
use crate::error::Result;

/// \[DESIGN\] The deepest a command or expression nests. The shipped
/// programs nest a few tens deep.
const MAX_NESTING: usize = 200;

/// The name of the routine a stemmer runs.
const STEM: &str = "stem";

/// What a name declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Integer,
    Boolean,
    String,
    Routine,
    Grouping,
}

/// The declarations, by key word, and what each declares. An external is
/// a routine its caller may run.
const DECLARATIONS: [(Key, Kind); 6] = [
    (Key::Integers, Kind::Integer),
    (Key::Booleans, Kind::Boolean),
    (Key::Strings, Kind::String),
    (Key::Routines, Kind::Routine),
    (Key::Externals, Kind::Routine),
    (Key::Groupings, Kind::Grouping),
];

/// The parser's state.
struct Parser {
    tokens: Vec<Lexed>,
    next: usize,
    /// Each name declared: its kind and its place among those of its kind.
    names: HashMap<String, (Kind, usize)>,
    routines: Vec<Option<Routine>>,
    groupings: Vec<Option<Vec<char>>>,
    amongs: Vec<Option<Among>>,
    integers: usize,
    booleans: usize,
    strings: usize,
    /// Whether definitions are read inside `backwardmode`.
    backward: bool,
    /// The among of a `substring` read, whose `among` is still to come.
    substring: Option<usize>,
    /// How deep the command or expression being read nests.
    depth: usize,
}

/// The program the tokens `tokens` write.
///
/// # Errors
///
/// SQLCODE -916 if they are not a Snowball program this reads.
pub(super) fn parse(tokens: Vec<Lexed>) -> Result<Program> {
    let mut parser = Parser {
        tokens,
        next: 0,
        names: HashMap::new(),
        routines: Vec::new(),
        groupings: Vec::new(),
        amongs: Vec::new(),
        integers: 0,
        booleans: 0,
        strings: 0,
        backward: false,
        substring: None,
        depth: 0,
    };
    parser.program(false)?;
    parser.finish()
}

impl Parser {
    /// The line of the next token, or of the last.
    fn line(&self) -> u32 {
        self.tokens.get(self.next).or_else(|| self.tokens.last()).map_or(0, |lexed| lexed.line)
    }

    /// The next token.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.next).map(|lexed| &lexed.token)
    }

    /// The next token, taken.
    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.next).map(|lexed| lexed.token.clone());
        self.next = self.next.saturating_add(1);
        token
    }

    /// Whether the symbol `sym` is next, taking it if it is.
    fn accept_sym(&mut self, sym: Sym) -> bool {
        let found = self.peek() == Some(&Token::Sym(sym));
        if found {
            self.next = self.next.saturating_add(1);
        }
        found
    }

    /// Whether the key word `key` is next, taking it if it is.
    fn accept_key(&mut self, key: Key) -> bool {
        let found = self.peek() == Some(&Token::Key(key));
        if found {
            self.next = self.next.saturating_add(1);
        }
        found
    }

    /// The symbol `sym`, taken.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if something else is next.
    fn expect_sym(&mut self, sym: Sym) -> Result<()> {
        if self.accept_sym(sym) { Ok(()) } else { Err(damaged(self.line(), &format!("expected {sym:?}"))) }
    }

    /// The name next, taken.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if something else is next.
    fn expect_name(&mut self) -> Result<String> {
        match self.advance() {
            Some(Token::Name(name)) => Ok(name),
            Some(Token::Key(_) | Token::Number(_) | Token::Text(_) | Token::Sym(_)) | None => Err(damaged(self.line(), "expected a name")),
        }
    }

    /// The place of `name`, declared of `kind`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not declared, or not of that kind.
    fn resolve(&self, name: &str, kind: Kind) -> Result<usize> {
        let found = self.names.get(name).filter(|(declared, _)| *declared == kind);
        found.map(|(_, place)| *place).ok_or_else(|| damaged(self.line(), &format!("{name} is not declared a {kind:?}")))
    }

    /// A command or expression begun, its nesting counted.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 past `MAX_NESTING`.
    fn enter(&mut self) -> Result<()> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_NESTING {
            return Err(damaged(self.line(), "commands nest too deep"));
        }
        Ok(())
    }

    /// A command or expression ended.
    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Declarations and definitions, to the end or, `nested`, to the `)`
    /// closing a `backwardmode` block.
    fn program(&mut self, nested: bool) -> Result<()> {
        while let Some(token) = self.advance() {
            match token {
                Token::Key(Key::Define) => self.define()?,
                Token::Key(Key::Backwardmode) => self.backwardmode()?,
                Token::Key(key) => self.declare(key)?,
                Token::Sym(Sym::Ket) => {
                    if nested {
                        return Ok(());
                    }
                    return Err(damaged(self.line(), "a ) closes nothing"));
                }
                Token::Name(_) | Token::Number(_) | Token::Text(_) | Token::Sym(_) => {
                    return Err(damaged(self.line(), "expected a declaration or definition"));
                }
            }
        }
        if nested { Err(damaged(self.line(), "backwardmode is not closed")) } else { Ok(()) }
    }

    /// The declaration `key` begins: names in parentheses.
    fn declare(&mut self, key: Key) -> Result<()> {
        let Some((_, kind)) = DECLARATIONS.iter().find(|(declares, _)| *declares == key) else {
            return Err(damaged(self.line(), &format!("{key:?} begins no declaration")));
        };
        self.expect_sym(Sym::Bra)?;
        while !self.accept_sym(Sym::Ket) {
            let name = self.expect_name()?;
            let place = self.declared(*kind);
            if self.names.insert(name.clone(), (*kind, place)).is_some() {
                return Err(damaged(self.line(), &format!("{name} is declared twice")));
            }
        }
        Ok(())
    }

    /// The place of a name of `kind` declared now, its slot made.
    fn declared(&mut self, kind: Kind) -> usize {
        let count = match kind {
            Kind::Integer => &mut self.integers,
            Kind::Boolean => &mut self.booleans,
            Kind::String => &mut self.strings,
            Kind::Routine => {
                self.routines.push(None);
                return self.routines.len().saturating_sub(1);
            }
            Kind::Grouping => {
                self.groupings.push(None);
                return self.groupings.len().saturating_sub(1);
            }
        };
        let place = *count;
        *count = count.saturating_add(1);
        place
    }

    /// `backwardmode ( ... )`: definitions of routines run backward.
    fn backwardmode(&mut self) -> Result<()> {
        self.expect_sym(Sym::Bra)?;
        self.backward = true;
        self.program(true)?;
        self.backward = false;
        Ok(())
    }

    /// `define`: a routine, `define r as C`, or a grouping, `define g` and
    /// strings and groupings joined by `+` and `-`.
    fn define(&mut self) -> Result<()> {
        let name = self.expect_name()?;
        if let Ok(place) = self.resolve(&name, Kind::Grouping) {
            let chars = self.grouping()?;
            if let Some(slot) = self.groupings.get_mut(place) {
                *slot = Some(chars);
            }
            return Ok(());
        }
        let place = self.resolve(&name, Kind::Routine)?;
        if !self.accept_key(Key::As) {
            return Err(damaged(self.line(), "a routine is defined with as"));
        }
        let body = self.command()?;
        if self.substring.is_some() {
            return Err(damaged(self.line(), "a substring has no among"));
        }
        if let Some(slot) = self.routines.get_mut(place) {
            *slot = Some(Routine { body, backward: self.backward });
        }
        Ok(())
    }

    /// A grouping's characters: its parts joined, sorted.
    fn grouping(&mut self) -> Result<Vec<char>> {
        let mut chars = self.grouping_part()?;
        loop {
            if self.accept_sym(Sym::Plus) {
                chars.extend(self.grouping_part()?);
            } else if self.accept_sym(Sym::Minus) {
                let taken = self.grouping_part()?;
                chars.retain(|c| !taken.contains(c));
            } else {
                break;
            }
        }
        chars.sort_unstable();
        chars.dedup();
        Ok(chars)
    }

    /// A part of a grouping: a literal string's characters, or a grouping
    /// defined already.
    fn grouping_part(&mut self) -> Result<Vec<char>> {
        match self.advance() {
            Some(Token::Text(text)) => Ok(text),
            Some(Token::Name(name)) => {
                let place = self.resolve(&name, Kind::Grouping)?;
                self.groupings.get(place).cloned().flatten().ok_or_else(|| damaged(self.line(), &format!("{name} is used before it is defined")))
            }
            Some(Token::Key(_) | Token::Number(_) | Token::Sym(_)) | None => Err(damaged(self.line(), "expected a string or grouping")),
        }
    }

    /// The program, every routine, grouping and among defined.
    fn finish(self) -> Result<Program> {
        let line = self.line();
        let undefined = |what: &str| damaged(line, &format!("a {what} is declared but not defined"));
        let routines = self.routines.into_iter().map(|r| r.ok_or_else(|| undefined("routine"))).collect::<Result<Vec<_>>>()?;
        let groupings = self.groupings.into_iter().map(|g| g.ok_or_else(|| undefined("grouping"))).collect::<Result<Vec<_>>>()?;
        let amongs = self.amongs.into_iter().map(|a| a.ok_or_else(|| undefined("among"))).collect::<Result<Vec<_>>>()?;
        let stem = match self.names.get(STEM) {
            Some((Kind::Routine, place)) => *place,
            Some(_) | None => return Err(damaged(line, "the program has no routine stem")),
        };
        Ok(Program { routines, groupings, amongs, integers: self.integers, booleans: self.booleans, strings: self.strings, stem })
    }

}
