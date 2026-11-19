//! \[EXTENSION\] The `<locking clause>` of a query (RULES.md D185) and the
//! `<lock table statement>` (RULES.md D186). LOCK, SHARE, NOWAIT, SKIP,
//! LOCKED and the words of a mode are no key words, read as these only
//! here.

use super::Parser;
use crate::ast::{LockTable, LockTableMode, LockWaits, LockingClause, Updatability};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The word that begins a `<lock table statement>`.
pub(crate) const LOCK: &str = "LOCK";
/// The word of a lock FOR SHARE.
const SHARE: &str = "SHARE";
/// The word of a lock that refuses at once.
const NOWAIT: &str = "NOWAIT";
/// The words of a lock that passes over rows held.
const SKIP: &str = "SKIP";
const LOCKED: &str = "LOCKED";
/// The word after a `<lock table mode>`.
const MODE: &str = "MODE";

/// Whether the tokens `first` and `second` begin a `<lock table
/// statement>`: LOCK, then TABLE, ONLY, or a table's name.
pub(crate) fn begins_lock_table(first: Option<&Tok>, second: Option<&Tok>) -> bool {
    let lock = matches!(first, Some(Tok::Identifier(w)) if w == LOCK);
    lock && matches!(second, Some(Tok::KeyWord(kw::TABLE | kw::ONLY) | Tok::Identifier(_)))
}

impl Parser {
    /// \[EXTENSION\] An `<updatability clause>` or a `<locking clause>`, if
    /// one is next: FOR SHARE, or FOR UPDATE then NOWAIT or SKIP LOCKED,
    /// is a locking clause; FOR UPDATE alone an updatability clause, which
    /// a query run directly or prepared reads as a locking clause.
    pub(super) fn updatability_or_locking(&mut self) -> Result<(Option<Updatability>, Option<LockingClause>)> {
        let at = self.here();
        if self.is_kw(kw::FOR) && matches!(self.peek_at(1), Tok::Identifier(w) if w == SHARE) {
            self.advance();
            self.advance();
            let of = self.lock_of()?;
            let waits = self.lock_waits()?.unwrap_or(LockWaits::Wait);
            return Ok((None, Some(LockingClause { exclusive: false, of, waits, at })));
        }
        let updatability = self.updatability_clause()?;
        let Some(waits) = self.lock_waits()? else { return Ok((updatability, None)) };
        let Some(Updatability::Update(of)) = updatability else {
            return Err(self.syntax("[EXTENSION] NOWAIT and SKIP LOCKED follow FOR UPDATE or FOR SHARE (RULES.md D185)"));
        };
        Ok((None, Some(LockingClause { exclusive: true, of, waits, at })))
    }

    /// OF `<table or query name>` \[{, ...}...\], if written.
    fn lock_of(&mut self) -> Result<Option<Vec<(String, usize)>>> {
        if !self.accept_kw(kw::OF) {
            return Ok(None);
        }
        let mut names = Vec::new();
        loop {
            let at = self.here();
            names.push((self.identifier()?, at));
            if !self.accept_delim(delim::COMMA) {
                return Ok(Some(names));
            }
        }
    }

    /// NOWAIT or SKIP LOCKED, if written.
    fn lock_waits(&mut self) -> Result<Option<LockWaits>> {
        if self.accept_word(NOWAIT) {
            return Ok(Some(LockWaits::NoWait));
        }
        if self.accept_word(SKIP) {
            self.expect_word(LOCKED)?;
            return Ok(Some(LockWaits::SkipLocked));
        }
        Ok(None)
    }

    /// \[EXTENSION\] `<lock table statement>` ::= LOCK \[TABLE\] `<target
    /// table>` \[{, `<target table>`}...\] \[IN `<lock table mode>` MODE\]
    /// \[NOWAIT\] (RULES.md D186).
    pub(super) fn lock_table_statement(&mut self) -> Result<LockTable> {
        self.expect_word(LOCK)?;
        self.accept_kw(kw::TABLE);
        let mut tables = Vec::new();
        loop {
            let at = self.here();
            let (table, only) = self.target_table()?;
            tables.push((table, only, at));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        let mode = if self.accept_kw(kw::IN) { self.lock_table_mode()? } else { LockTableMode::AccessExclusive };
        let nowait = self.accept_word(NOWAIT);
        Ok(LockTable { tables, mode, nowait })
    }

    /// `<lock table mode>` MODE.
    fn lock_table_mode(&mut self) -> Result<LockTableMode> {
        for (mode, words) in LockTableMode::NAMED {
            if self.words_next(words) {
                for _ in words {
                    self.advance();
                }
                self.expect_word(MODE)?;
                return Ok(mode);
            }
        }
        Err(self.syntax("[EXTENSION] expected a <lock table mode>: ACCESS SHARE, ROW SHARE, ROW EXCLUSIVE, SHARE UPDATE EXCLUSIVE, SHARE, SHARE ROW EXCLUSIVE, EXCLUSIVE or ACCESS EXCLUSIVE (RULES.md D186)"))
    }

    /// Whether `words`, each a word or a key word, are next, then MODE.
    fn words_next(&self, words: &[&str]) -> bool {
        let named = |n: usize, word: &str| match self.peek_at(n) {
            Tok::Identifier(w) => w == word,
            Tok::KeyWord(k) => *k == word,
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => false,
        };
        words.iter().enumerate().all(|(n, word)| named(n, word)) && named(words.len(), MODE)
    }
}
