//! \[EXTENSION\] The statements of backups (RULES.md D160, D164, D166).
//! ISO/IEC 9075:1992 has none. In its notation:
//!
//! ```text
//! <backup statement> ::=
//!       BACKUP DATABASE TO <character string literal>
//!       [ INCREMENTAL [ FROM <character string literal> ] ]
//!       [ LABEL <character string literal> ]
//! <restore point statement> ::=
//!       CREATE RESTORE POINT <identifier>
//! <rescan statement> ::=
//!       RESCAN BACKUP TARGET <character string literal>
//! ```
//!
//! BACKUP, DATABASE, INCREMENTAL, LABEL, RESTORE, POINT, RESCAN and TARGET
//! are no standard's `<reserved word>`s, and stay `<regular identifier>`s
//! everywhere else, matched as words here alone.

use super::Parser;
use crate::ast::{SqlStatement, UserStatement};
use crate::error::Result;
use crate::lexer::{Tok, kw};

/// \[EXTENSION\] The word that begins a `<backup statement>`, and names
/// the target of a `<rescan statement>`.
const BACKUP: &str = "BACKUP";
/// \[EXTENSION\] The word after BACKUP.
const DATABASE: &str = "DATABASE";
/// \[EXTENSION\] The word of an incremental backup.
const INCREMENTAL: &str = "INCREMENTAL";
/// \[EXTENSION\] The word before a backup's label.
const LABEL: &str = "LABEL";
/// \[EXTENSION\] The word after CREATE in a `<restore point statement>`.
const RESTORE: &str = "RESTORE";
/// \[EXTENSION\] The word after RESTORE.
const POINT: &str = "POINT";
/// \[EXTENSION\] The word that begins a `<rescan statement>`.
const RESCAN: &str = "RESCAN";
/// \[EXTENSION\] The word after RESCAN BACKUP.
const TARGET: &str = "TARGET";

/// Whether the tokens `first` and `second` begin a `<backup statement>` or
/// a `<rescan statement>`, not a query: BACKUP DATABASE, or RESCAN BACKUP,
/// no `<joined table>`, which a key word would follow.
pub(super) fn begins_backup_statement(first: Option<&Tok>, second: Option<&Tok>) -> bool {
    let (Some(Tok::Identifier(a)), Some(Tok::Identifier(b))) = (first, second) else { return false };
    (a == BACKUP && b == DATABASE) || (a == RESCAN && b == BACKUP)
}

impl Parser {
    /// Whether a statement of backups begins here.
    pub(super) fn at_backup_statement(&self) -> bool {
        let restore = self.is_kw(kw::CREATE) && *self.peek_at(1) == Tok::Identifier(RESTORE.to_owned());
        restore || begins_backup_statement(Some(self.peek()), Some(self.peek_at(1)))
    }

    /// The statement of backups that begins here (`at_backup_statement`).
    pub(super) fn backup_statement(&mut self) -> Result<SqlStatement> {
        if self.accept_kw(kw::CREATE) {
            self.expect_word(RESTORE)?;
            self.expect_word(POINT)?;
            let at = self.here();
            return Ok(SqlStatement::User(UserStatement::RestorePoint { name: (self.identifier()?, at) }));
        }
        if self.accept_word(RESCAN) {
            self.expect_word(BACKUP)?;
            self.expect_word(TARGET)?;
            let location = self.character_string_literal("backup target's location")?;
            return Ok(SqlStatement::User(UserStatement::Rescan { location }));
        }
        self.expect_word(BACKUP)?;
        self.expect_word(DATABASE)?;
        self.expect_kw(kw::TO)?;
        let location = self.character_string_literal("backup target's location")?;
        let incremental = if self.accept_word(INCREMENTAL) {
            Some(if self.accept_kw(kw::FROM) { self.character_string_literal("backup's id")? } else { String::new() })
        } else {
            None
        };
        let label = if self.accept_word(LABEL) { self.character_string_literal("backup's label")? } else { String::new() };
        Ok(SqlStatement::User(UserStatement::Backup { location, incremental, label }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether the statement `sql` begins is one of backups.
    fn at(sql: &str) -> bool {
        Parser::new(sql).unwrap().at_backup_statement()
    }

    /// Verifies: [EXTENSION] RULES.md D160, D164, D166, R-TEST-8: a
    /// statement of backups begins BACKUP DATABASE, by its first word (pair
    /// 1, 2) and its second (1, 3); RESCAN BACKUP, by its first (4, 5) and
    /// its second (4, 6); or CREATE RESTORE, by CREATE (7, 8) and RESTORE
    /// (7, 9).
    #[test]
    fn its_beginning_shows_mcdc() {
        assert!(at("BACKUP DATABASE TO 'a'"), "1");
        assert!(!at("BACKUPS DATABASE TO 'a'"), "2");
        assert!(!at("BACKUP DATA TO 'a'"), "3");
        assert!(at("RESCAN BACKUP TARGET 'a'"), "4");
        assert!(!at("RESCANS BACKUP TARGET 'a'"), "5");
        assert!(!at("RESCAN BACKUPS TARGET 'a'"), "6");
        assert!(at("CREATE RESTORE POINT P"), "7");
        assert!(!at("DROP RESTORE POINT P"), "8");
        assert!(!at("CREATE RESTORED POINT P"), "9");
    }
}
