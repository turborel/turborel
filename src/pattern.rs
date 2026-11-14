//! [ISO/IEC 9075-2:2023 §9.41] Row pattern recognition in a sequence of
//! rows: the preferred row pattern match of a `<row pattern>` that begins
//! at a given row, if one does.
//!
//! A pattern's primary row pattern variables are numbered; which rows a
//! variable may be mapped to is judged by the caller, given the rows mapped
//! so far, since a `<row pattern definition search condition>` may read
//! them (PREV, a running aggregate, another variable's rows) [§4.22].
//! Since the judgement depends on what is mapped before, no state of an
//! automaton stands for all the matches that reach it, so the search tries
//! the parenthesized language's strings in the order of preferment \[GR6\],
//! the first candidate found being the preferred match \[GR10\]. Its steps
//! are counted, past `MAX_STEPS` an error.
//!
//! `pattern/compile.rs` turns a pattern into instructions whose forks list
//! the preferred way first; `pattern/search.rs` follows them, keeping the
//! other ways on a stack of its own.

mod compile;
mod search;

use crate::error::{Code, Result, SqlError};
use compile::compile;
use search::Search;

/// [ISO/IEC 9075-2:2023 §7.9] A `<row pattern>` of numbered primary row
/// pattern variables, each quantifier read as `{ n, m }` or `{ n, }` with
/// its reluctance [§7.9 SR19].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// The primary row pattern variable numbered so.
    Variable(usize),
    /// `^`: the start of the row sequence.
    Start,
    /// `$`: the end of the row sequence.
    End,
    /// `( )`.
    Empty,
    /// Its patterns one after another, in order.
    Concat(Vec<Pattern>),
    /// One of its patterns, the earlier preferred \[GR6e\].
    Alternation(Vec<Pattern>),
    /// Its pattern repeated from `min` to `max` times, none unbounded,
    /// fewer preferred if `reluctant` [GR6b, GR6c]. A `max` below `min`,
    /// which [§7.9 SR20] refuses, is read as `min`, so the bounds always
    /// allow an iteration while fewer than `min` are done.
    Repeat {
        /// The pattern repeated.
        pattern: Box<Pattern>,
        /// The least number of times.
        min: u32,
        /// The most, none if unbounded.
        max: Option<u32>,
        /// A reluctant quantifier.
        reluctant: bool,
    },
    /// `{- -}`: its rows left out of ALL ROWS PER MATCH's output.
    Excluded(Box<Pattern>),
    /// PERMUTE: the alternation of the concatenations of its patterns in
    /// each order, in the lexicographic order of the permutations [§7.9
    /// SR21].
    Permute(Vec<Pattern>),
}

/// [ISO/IEC 9075-2:2023 §9.41 GR8] A row of a match: the primary row
/// pattern variable it is mapped to, and whether a `{- -}` holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mapped {
    /// The primary row pattern variable.
    pub variable: usize,
    /// It is between `{-` and `-}`.
    pub excluded: bool,
}

/// \[DESIGN\] The most steps one search may take, each an attempt to map a
/// row, an alternative tried, or a quantifier's choice or iteration, before
/// it fails, so a pattern whose alternatives grow exponentially with the
/// rows cannot run for hours.
pub const MAX_STEPS: u64 = 10_000_000;

/// A `Pattern` made ready to search with.
#[derive(Debug, Clone)]
pub struct Program {
    pattern: Pattern,
    code: compile::Code,
}

impl Program {
    /// `pattern` made ready to search with.
    ///
    /// # Errors
    ///
    /// \[DESIGN\] SQLCODE -900 for a pattern beyond this implementation's
    /// limits: PERMUTE of more than `compile::MAX_PERMUTE` patterns, more
    /// than `compile::MAX_INSTRUCTIONS` instructions, or patterns nested
    /// more than `compile::MAX_DEPTH` deep.
    pub fn new(pattern: &Pattern) -> Result<Program> {
        Ok(Program { pattern: pattern.clone(), code: compile(pattern)? })
    }

    /// The pattern it searches with.
    pub fn pattern(&self) -> &Pattern {
        &self.pattern
    }

    /// [ISO/IEC 9075-2:2023 §9.41 GR7 to GR10] The preferred match of the
    /// rows `first` to `end`, excluded, that begins at row `start`, none if
    /// there is no candidate. A match lists the variable each row from
    /// `start` on is mapped to, and an empty match lists none. `^` matches
    /// before `first` alone and `$` after the last row before `end` alone.
    /// `holds` judges whether row `start + path.len() - 1` may be mapped to
    /// the variable of `path`'s last element, `path` being the rows mapped
    /// so far from `start` with that row last.
    ///
    /// # Errors
    ///
    /// `holds`'s; \[DESIGN\] past `MAX_STEPS`.
    pub fn find(
        &self,
        (first, start, end): (usize, usize, usize),
        holds: &mut impl FnMut(usize, &[Mapped]) -> Result<bool>,
    ) -> Result<Option<Vec<Mapped>>> {
        Search::new(&self.code, (first, start, end)).run(holds)
    }
}

/// The error of a pattern or search beyond turborel's limits, `why`.
fn beyond(why: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] the row pattern is beyond turborel's limits: {why}"))
}

#[cfg(test)]
mod tests;
