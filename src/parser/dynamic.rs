//! [ISO/IEC 9075:1992 §17] Dynamic SQL in SQL text: its statements' grammar,
//! read in full, so that each is refused as valid but not implemented
//! (SQLCODE -900, RULES.md D18), and SQL that breaks its Format is a syntax
//! error. A program prepares and executes statements through the Rust API
//! and the protocol instead. Split from parser.rs (RULES.md R-SIZE-2); the
//! methods extend the same `Parser`.

use super::Parser;
use crate::ast::SqlStatement;
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

/// [ISO/IEC 9075:1992 §17.4] The `<descriptor item name>`s.
const ITEM_NAMES: &[&str] = &[
    "TYPE",
    "LENGTH",
    "OCTET_LENGTH",
    "RETURNED_LENGTH",
    "RETURNED_OCTET_LENGTH",
    "PRECISION",
    "SCALE",
    "DATETIME_INTERVAL_CODE",
    "DATETIME_INTERVAL_PRECISION",
    "NULLABLE",
    "INDICATOR",
    "DATA",
    "NAME",
    "UNNAMED",
    "COLLATION_CATALOG",
    "COLLATION_SCHEMA",
    "COLLATION_NAME",
    "CHARACTER_SET_CATALOG",
    "CHARACTER_SET_SCHEMA",
    "CHARACTER_SET_NAME",
];

impl Parser {
    /// SQLCODE -900 at the statement's end: `what`, valid SQL-92 dynamic
    /// SQL, which SQL text does not take; the statement it is read as is
    /// never run.
    fn dynamic_refused(&mut self, at: usize, what: &str) -> SqlStatement {
        let refused = SqlError::at(
            Code::Unsupported,
            at,
            format!(
                "[DESIGN] {what} is valid SQL, but dynamic SQL in SQL text is not implemented: prepare through the API or the protocol (RULES.md D18, D45)"
            ),
        );
        self.refuse_later(refused);
        SqlStatement::Commit
    }

    /// A `<parameter name>` [§5.4], `:` and an `<identifier>`, or one
    /// written as SQL-86 writes it, an `<identifier>` alone, with its
    /// indicator if written: as a `<simple target specification>` or
    /// `<target specification>` of dynamic SQL is.
    fn dynamic_target(&mut self) -> Result<()> {
        self.accept_delim(delim::COLON);
        self.identifier()?;
        let indicator = self.accept_kw(kw::INDICATOR);
        if indicator || self.is_delim(delim::COLON) || matches!(self.peek(), Tok::Identifier(_)) {
            self.accept_delim(delim::COLON);
            self.identifier()?;
        }
        Ok(())
    }

    /// [§6.2] A `<simple value specification>` of dynamic SQL: a parameter,
    /// as `dynamic_target` reads one, or a `<literal>`.
    fn dynamic_value(&mut self) -> Result<()> {
        if self.is_delim(delim::COLON) || matches!(self.peek(), Tok::Identifier(_)) {
            return self.dynamic_target();
        }
        self.literal().map(drop)
    }

    /// [§5.4] `<descriptor name>`, `<extended statement name>` or `<extended
    /// cursor name>` ::= \[GLOBAL | LOCAL\] `<simple value specification>`.
    fn extended_name(&mut self) -> Result<()> {
        if !self.accept_kw(kw::GLOBAL) {
            self.accept_kw(kw::LOCAL);
        }
        self.dynamic_value()
    }

    /// [§5.4] `<SQL statement name>` ::= `<statement name>` | `<extended
    /// statement name>`; so too `<dynamic cursor name>`, of a `<cursor
    /// name>`: an `<identifier>`, or an extended name.
    fn statement_name(&mut self) -> Result<()> {
        if matches!(self.peek(), Tok::Identifier(_)) {
            return self.identifier().map(drop);
        }
        self.extended_name()
    }

    /// Whether an extended name is next: GLOBAL, LOCAL, a parameter or a
    /// literal, not an `<identifier>`.
    fn at_extended_name(&self) -> bool {
        matches!(self.peek(), Tok::KeyWord(kw::GLOBAL | kw::LOCAL) | Tok::CharacterString(_) | Tok::Delimiter(delim::COLON))
    }

    /// [§17.9] `<using clause>` ::= {USING | INTO} SQL DESCRIPTOR
    /// `<descriptor name>` | {USING | INTO} `<argument>` \[{, `<argument>`}...\],
    /// if next.
    fn using_clause(&mut self) -> Result<bool> {
        if !(self.accept_kw(kw::USING) || self.accept_kw(kw::INTO)) {
            return Ok(false);
        }
        if self.accept_kw(kw::SQL) {
            self.expect_kw(kw::DESCRIPTOR)?;
            self.extended_name()?;
            return Ok(true);
        }
        self.dynamic_target()?;
        while self.accept_delim(delim::COMMA) {
            self.dynamic_target()?;
        }
        Ok(true)
    }

    /// [§17.4] A `<descriptor item name>`.
    fn descriptor_item(&mut self) -> Result<()> {
        let word = match self.peek() {
            Tok::Identifier(w) => w.clone(),
            Tok::KeyWord(w) => (*w).to_owned(),
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Introducer(_)
            | Tok::Delimiter(_)
            | Tok::End => String::new(),
        };
        if !ITEM_NAMES.contains(&word.as_str()) {
            return Err(self.syntax("expected a <descriptor item name>"));
        }
        self.advance();
        Ok(())
    }

    /// [§17.2, §17.3, §17.13] ALLOCATE DESCRIPTOR `<descriptor name>`
    /// \[WITH MAX `<occurrences>`\]; DEALLOCATE DESCRIPTOR `<descriptor
    /// name>`; DEALLOCATE PREPARE `<SQL statement name>` [§17.7]; ALLOCATE
    /// `<extended cursor name>` \[INSENSITIVE\] \[SCROLL\] CURSOR FOR
    /// `<extended statement name>`: read, then refused.
    pub(super) fn allocation(&mut self) -> Result<SqlStatement> {
        let at = self.here();
        if self.accept_kw(kw::DEALLOCATE) {
            if self.accept_kw(kw::PREPARE) {
                self.statement_name()?;
                return Ok(self.dynamic_refused(at, "DEALLOCATE PREPARE"));
            }
            self.expect_kw(kw::DESCRIPTOR)?;
            self.extended_name()?;
            return Ok(self.dynamic_refused(at, "DEALLOCATE DESCRIPTOR"));
        }
        self.expect_kw(kw::ALLOCATE)?;
        if self.accept_kw(kw::DESCRIPTOR) {
            self.extended_name()?;
            if self.accept_kw(kw::WITH) {
                self.expect_kw(kw::MAX)?;
                self.dynamic_value()?;
            }
            return Ok(self.dynamic_refused(at, "ALLOCATE DESCRIPTOR"));
        }
        self.extended_name()?;
        self.accept_kw(kw::INSENSITIVE);
        self.accept_kw(kw::SCROLL);
        self.expect_kw(kw::CURSOR)?;
        self.expect_kw(kw::FOR)?;
        self.extended_name()?;
        Ok(self.dynamic_refused(at, "ALLOCATE CURSOR"))
    }

    /// [§17.4] GET DESCRIPTOR `<descriptor name>` {`<simple target
    /// specification>` = COUNT | VALUE `<item number>` `<simple target
    /// specification>` = `<descriptor item name>` \[{, ...}...\]}; [§17.5]
    /// SET DESCRIPTOR `<descriptor name>` {COUNT = `<simple value
    /// specification>` | VALUE `<item number>` `<descriptor item name>` =
    /// `<simple value specification>` \[{, ...}...\]}: after GET or SET,
    /// read, then refused.
    pub(super) fn descriptor_statement(&mut self, get: bool) -> Result<SqlStatement> {
        let at = self.here();
        self.expect_kw(kw::DESCRIPTOR)?;
        self.extended_name()?;
        if self.accept_kw(kw::VALUE) {
            self.dynamic_value()?;
            loop {
                if get {
                    self.dynamic_target()?;
                    self.expect_delim(delim::EQUALS)?;
                    self.descriptor_item()?;
                } else {
                    self.descriptor_item()?;
                    self.expect_delim(delim::EQUALS)?;
                    self.dynamic_value()?;
                }
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
        } else if get {
            self.dynamic_target()?;
            self.expect_delim(delim::EQUALS)?;
            self.expect_kw(kw::COUNT)?;
        } else {
            self.expect_kw(kw::COUNT)?;
            self.expect_delim(delim::EQUALS)?;
            self.dynamic_value()?;
        }
        Ok(self.dynamic_refused(at, if get { "GET DESCRIPTOR" } else { "SET DESCRIPTOR" }))
    }

    /// [§17.6] PREPARE `<SQL statement name>` FROM `<SQL statement
    /// variable>`; [§17.8] DESCRIBE \[INPUT | OUTPUT\] `<SQL statement name>`
    /// `<using descriptor>`; [§17.10] EXECUTE `<SQL statement name>`
    /// \[`<using clause>`\] \[`<using clause>`\]; [§17.11] EXECUTE IMMEDIATE
    /// `<SQL statement variable>`: read, then refused.
    pub(super) fn dynamic_statement(&mut self) -> Result<SqlStatement> {
        let at = self.here();
        if self.accept_kw(kw::PREPARE) {
            self.statement_name()?;
            self.expect_kw(kw::FROM)?;
            self.dynamic_value()?;
            return Ok(self.dynamic_refused(at, "PREPARE"));
        }
        if self.accept_kw(kw::DESCRIBE) {
            if !self.accept_kw(kw::INPUT) {
                self.accept_kw(kw::OUTPUT);
            }
            self.statement_name()?;
            if !(self.is_kw(kw::USING) || self.is_kw(kw::INTO)) || !self.next_is_kw(kw::SQL) {
                return Err(self.syntax("expected USING SQL DESCRIPTOR or INTO SQL DESCRIPTOR"));
            }
            self.using_clause()?;
            return Ok(self.dynamic_refused(at, "DESCRIBE"));
        }
        self.expect_kw(kw::EXECUTE)?;
        if self.accept_kw(kw::IMMEDIATE) {
            self.dynamic_value()?;
            return Ok(self.dynamic_refused(at, "EXECUTE IMMEDIATE"));
        }
        self.statement_name()?;
        if self.using_clause()? {
            self.using_clause()?;
        }
        Ok(self.dynamic_refused(at, "EXECUTE"))
    }

    /// [§17.14, §17.16] OPEN or CLOSE of an `<extended cursor name>`, after
    /// the key word, OPEN's with a `<using clause>` if written: read, then
    /// refused; none if an `<identifier>`, a cursor's name, is next.
    pub(super) fn dynamic_open_or_close(&mut self, open: bool) -> Result<Option<SqlStatement>> {
        let at = self.here();
        if !self.at_extended_name() {
            return Ok(None);
        }
        self.extended_name()?;
        if open {
            self.using_clause()?;
        }
        Ok(Some(self.dynamic_refused(at, if open { "a dynamic OPEN" } else { "a dynamic CLOSE" })))
    }

    /// [§17.14] OPEN `<cursor name>` `<using clause>`, a cursor's name read:
    /// read, then refused, if a using clause is next.
    pub(super) fn dynamic_open_using(&mut self) -> Result<bool> {
        let at = self.here();
        if !self.using_clause()? {
            return Ok(false);
        }
        self.dynamic_refused(at, "a dynamic OPEN");
        Ok(true)
    }

    /// [§17.15] The `<dynamic cursor name>` and `<using clause>` of a
    /// dynamic FETCH, after FETCH \[\[`<fetch orientation>`\] FROM\]: an
    /// extended cursor name, or a cursor's with USING, or INTO SQL
    /// DESCRIPTOR; read, then refused, if one is next.
    pub(super) fn dynamic_fetch(&mut self) -> Result<bool> {
        let at = self.here();
        let extended = self.at_extended_name();
        let descriptor = matches!(self.peek(), Tok::Identifier(_))
            && (matches!(self.peek_at(1), Tok::KeyWord(kw::USING))
                || (matches!(self.peek_at(1), Tok::KeyWord(kw::INTO)) && matches!(self.peek_at(2), Tok::KeyWord(kw::SQL))));
        if !extended && !descriptor {
            return Ok(false);
        }
        self.statement_name()?;
        if !self.using_clause()? {
            return Err(self.syntax("expected a <using clause>"));
        }
        self.dynamic_refused(at, "a dynamic FETCH");
        Ok(true)
    }

    /// [§17.19, §17.20] WHERE CURRENT OF `<cursor name>`, of a positioned
    /// DELETE or UPDATE prepared with no table: read, then refused.
    pub(super) fn preparable_positioned(&mut self) -> Result<SqlStatement> {
        let at = self.here();
        self.expect_kw(kw::WHERE)?;
        self.expect_kw(kw::CURRENT)?;
        self.expect_kw(kw::OF)?;
        self.identifier()?;
        Ok(self.dynamic_refused(at, "a prepared positioned statement of no table"))
    }

    /// [§17.17, §17.18] WHERE CURRENT OF an `<extended cursor name>`, after
    /// CURRENT OF: read, then refused, if one is next.
    pub(super) fn dynamic_current_of(&mut self) -> Result<bool> {
        let at = self.here();
        if !self.at_extended_name() {
            return Ok(false);
        }
        self.extended_name()?;
        self.dynamic_refused(at, "a positioned statement of an extended cursor name");
        Ok(true)
    }
}
