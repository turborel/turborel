//! The Snowball stemmers [ISO/IEC 13249-2:2002 (CD, SC32 N0640)
//! §4.2.3.4]: a word's base reduced form in each language Snowball has an
//! algorithm for, by that algorithm's own program (snowballstem.org,
//! release 3.2.0), run by an interpreter of the Snowball language
//! (RULES.md D320).
//!
//! The programs are kept as Snowball wrote them, in `snowball/algorithms`,
//! under Snowball's BSD license, whose notice is `snowball/algorithms/
//! COPYING`. Each is read the first time its language stems a word and
//! kept for the process. The interpreter is the lexer (`lex.rs`), the
//! parser (`parse.rs`) and the machine (`run.rs`).

mod ast;
mod lex;
mod parse;
mod run;
#[cfg(test)]
mod tests;

use crate::error::{Code, Result, SqlError};

/// A Snowball program, read and ready to stem words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stemmer {
    program: ast::Program,
}

impl Stemmer {
    /// The stemmer the Snowball program `source` defines, its routine
    /// `stem` the one it runs.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `source` is not a Snowball program the interpreter
    /// reads, naming the line it fails at.
    pub fn compile(source: &str) -> Result<Stemmer> {
        Ok(Stemmer { program: parse::parse(lex::lex(source)?)? })
    }

    /// The stem of `word`: the word as the routine `stem` leaves it. A run
    /// that stops, as a damaged program's may, leaves the word as it is.
    pub fn stem(&self, word: &str) -> String {
        run::stem(&self.program, word)
    }
}

/// The error of a program the interpreter cannot read, at `line`.
fn damaged(line: u32, why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a Snowball program is damaged at line {line}: {why}"))
}
