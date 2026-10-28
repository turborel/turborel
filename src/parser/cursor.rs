//! Cursors' statements: `<declare cursor>` [X3.135-1986 §8.3; ISO/IEC
//! 9075:1992 §13.1], with its `<updatability clause>`, and `<fetch
//! statement>` [X3.135-1986 §8.6; ISO/IEC 9075:1992 §13.3]. Split from
//! parser.rs (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{DeclareCursor, FetchOrientation, SqlStatement, Updatability};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// [ISO/IEC 9075:1992 §13.1] `<declare cursor>` ::= DECLARE `<cursor
    /// name>` \[INSENSITIVE\] \[SCROLL\] CURSOR FOR `<cursor specification>`
    ///
    /// # Errors
    ///
    /// The text is not a `<declare cursor>`.
    pub fn declare_cursor(&mut self) -> Result<DeclareCursor> {
        self.expect_kw(kw::DECLARE)?;
        let name = self.identifier()?;
        let insensitive = self.accept_kw(kw::INSENSITIVE);
        let scroll = self.accept_kw(kw::SCROLL);
        self.expect_kw(kw::CURSOR)?;
        self.expect_kw(kw::FOR)?;
        // Kept as text and parsed at OPEN, where the procedure's parameters
        // are known [X3.135-1986 §8.3 SR2, §8.8 GR3a].
        let start = self.here();
        if *self.peek() == Tok::End {
            return Err(self.syntax("expected a <cursor specification>"));
        }
        let specification: String = self.src.get(start..).unwrap_or_default().iter().collect();
        Ok(DeclareCursor { name, insensitive, scroll, specification })
    }

    /// A statement beginning DECLARE: a `<temporary table declaration>`,
    /// a `<direct SQL data statement>` [ISO/IEC 9075:1992 §20.1]; a
    /// `<declare cursor>` is the module's, refused here.
    pub(super) fn declare_statement(&mut self) -> Result<SqlStatement> {
        if self.next_is_kw(kw::LOCAL) {
            return Ok(SqlStatement::DeclareTable(self.temporary_table_declaration()?));
        }
        Err(self
            .syntax("[X3.135-1986 §7.1, §7.3] a <declare cursor> belongs to the module, not a procedure; declare it with declare_cursor"))
    }

    /// [ISO/IEC 9075:1992 §13.1] `<updatability clause>` ::= FOR {READ ONLY
    /// | UPDATE \[OF `<column name list>`\]}, if one is next.
    pub(super) fn updatability_clause(&mut self) -> Result<Option<Updatability>> {
        if !self.accept_kw(kw::FOR) {
            return Ok(None);
        }
        if self.accept_kw(kw::READ) {
            self.expect_kw(kw::ONLY)?;
            return Ok(Some(Updatability::ReadOnly));
        }
        self.expect_kw(kw::UPDATE)?;
        if !self.accept_kw(kw::OF) {
            return Ok(Some(Updatability::Update(None)));
        }
        let mut columns = Vec::new();
        loop {
            let at = self.here();
            columns.push((self.identifier()?, at));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        Ok(Some(Updatability::Update(Some(columns))))
    }

    /// [ISO/IEC 9075:1992 §13.3] `<fetch statement>` ::= FETCH \[\[`<fetch
    /// orientation>`\] FROM\] `<cursor name>` INTO `<fetch target list>`
    pub(super) fn fetch_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::FETCH)?;
        let orientation = if self.accept_kw(kw::FROM) {
            FetchOrientation::Next
        } else {
            match self.fetch_orientation()? {
                Some(orientation) => {
                    self.expect_kw(kw::FROM)?;
                    orientation
                }
                None => FetchOrientation::Next,
            }
        };
        let cursor = self.identifier()?;
        self.expect_kw(kw::INTO)?;
        Ok(SqlStatement::Fetch { cursor, orientation, targets: self.target_list()? })
    }

    /// [ISO/IEC 9075:1992 §13.3] `<fetch orientation>` ::= NEXT | PRIOR |
    /// FIRST | LAST | {ABSOLUTE | RELATIVE} `<simple value specification>`,
    /// if one is next.
    fn fetch_orientation(&mut self) -> Result<Option<FetchOrientation>> {
        let orientation = if self.accept_kw(kw::NEXT) {
            FetchOrientation::Next
        } else if self.accept_kw(kw::PRIOR) {
            FetchOrientation::Prior
        } else if self.accept_kw(kw::FIRST) {
            FetchOrientation::First
        } else if self.accept_kw(kw::LAST) {
            FetchOrientation::Last
        } else if self.accept_kw(kw::ABSOLUTE) {
            FetchOrientation::Absolute(self.simple_value_specification()?)
        } else if self.accept_kw(kw::RELATIVE) {
            FetchOrientation::Relative(self.simple_value_specification()?)
        } else {
            return Ok(None);
        };
        Ok(Some(orientation))
    }
}
