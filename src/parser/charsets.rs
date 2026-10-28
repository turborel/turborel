//! [ISO/IEC 9075:1992 §5.3, §5.4, §10.4, §10.5, §11.1, §11.28-§11.33,
//! §11.36, §16.3] Character sets, collations and translations: their
//! grammar, read in full, so that SQL-92 that names any but SQL_TEXT, the
//! one character set there is, or a collation or a translation, is refused
//! as valid but never implemented (SQLCODE -900, RULES.md D32), and SQL
//! that breaks their Format is a syntax error. Naming SQL_TEXT is taken.
//! Split from parser.rs (RULES.md R-SIZE-2); the methods extend the same
//! `Parser`.

use super::Parser;
use crate::ast::{SqlStatement, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

/// The one character set [ISO/IEC 9075:1992 §4.2, §10.4 SR3; RULES.md D32].
const SQL_TEXT: &str = "SQL_TEXT";

/// The schema SQL_TEXT is described in [ISO/IEC 9075:1992 §21.3.23].
const ITS_SCHEMA: &str = "INFORMATION_SCHEMA";

/// Whether `name`, a `<character set name>`'s parts joined by periods,
/// names SQL_TEXT: alone, or of INFORMATION_SCHEMA, of any catalog.
fn is_sql_text(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    match parts.as_slice() {
        [one] => *one == SQL_TEXT,
        [schema, one] | [_, schema, one] => *schema == ITS_SCHEMA && *one == SQL_TEXT,
        _ => false,
    }
}

/// Whether `name` is a `<SQL language identifier>` [ISO/IEC 9075:1992 §5.4]:
/// a simple Latin letter, then letters, digits and underscores.
fn is_language_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// SQLCODE -900: `what`, valid SQL-92, names a character set other than
/// SQL_TEXT, a collation or a translation, which are never implemented.
fn never(at: usize, what: &str) -> SqlError {
    SqlError::at(
        Code::Unsupported,
        at,
        format!(
            "[DESIGN] {what} is valid SQL, but a character set other than SQL_TEXT, a collation and a translation are never implemented (RULES.md D32)"
        ),
    )
}

impl Parser {
    /// `name`, a character set's, at `at`: taken if SQL_TEXT; else refused
    /// once the statement is read.
    pub(super) fn only_sql_text(&mut self, at: usize, name: &str) {
        if !is_sql_text(name) {
            let refused = never(at, &format!("the character set {name}"));
            self.refuse_later(refused);
        }
    }

    /// [ISO/IEC 9075:1992 §5.4] `<character set name>` ::= \[`<schema name>`
    /// .\] `<SQL language identifier>`, `<schema name>` ::= \[`<catalog
    /// name>` .\] `<identifier>`: its parts joined by periods.
    pub(super) fn character_set_name(&mut self) -> Result<String> {
        let mut parts = vec![self.identifier()?];
        while parts.len() < 3 && self.accept_delim(delim::PERIOD) {
            parts.push(self.identifier()?);
        }
        if !parts.last().is_some_and(|p| is_language_identifier(p)) {
            return Err(self.syntax("a <character set name> ends in an <SQL language identifier>"));
        }
        Ok(parts.join("."))
    }

    /// [ISO/IEC 9075:1992 §5.4] `<qualified name>` ::= \[`<schema name>` .\]
    /// `<qualified identifier>`, as a `<collation name>` or `<translation
    /// name>` is: its parts joined by periods.
    fn qualified_name(&mut self) -> Result<String> {
        let mut parts = vec![self.identifier()?];
        while parts.len() < 3 && self.accept_delim(delim::PERIOD) {
            parts.push(self.identifier()?);
        }
        Ok(parts.join("."))
    }

    /// [ISO/IEC 9075:1992 §10.4] CHARACTER SET `<character set
    /// specification>`, if next: taken if it names SQL_TEXT, else refused
    /// once the statement is read.
    pub(super) fn character_set_clause(&mut self) -> Result<()> {
        if !(self.is_kw(kw::CHARACTER) && self.next_is_kw(kw::SET)) {
            return Ok(());
        }
        self.advance();
        self.advance();
        let at = self.here();
        let name = self.character_set_name()?;
        self.only_sql_text(at, &name);
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §10.5] `<collate clause>` ::= COLLATE `<collation
    /// name>`, if next: refused once the statement is read.
    pub(super) fn collate_clause(&mut self) -> Result<()> {
        let at = self.here();
        if !self.accept_kw(kw::COLLATE) {
            return Ok(());
        }
        let name = self.qualified_name()?;
        let refused = never(at, &format!("COLLATE {name}"));
        self.refuse_later(refused);
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §11.30] `<collation source>` ::= EXTERNAL ( '
    /// `<external collation name>` ' ) | `<schema collation name>` | DESC (
    /// `<collation name>` ) | DEFAULT | TRANSLATION `<translation name>`
    /// \[THEN COLLATION `<collation name>`\].
    fn collation_source(&mut self) -> Result<()> {
        if self.accept_kw(kw::EXTERNAL) {
            return self.external_name();
        }
        if self.accept_kw(kw::DESC) {
            self.expect_delim(delim::LEFT_PAREN)?;
            self.qualified_name()?;
            return self.expect_delim(delim::RIGHT_PAREN);
        }
        if self.accept_kw(kw::DEFAULT) {
            return Ok(());
        }
        if self.accept_kw(kw::TRANSLATION) {
            self.qualified_name()?;
            if self.accept_kw(kw::THEN) {
                self.expect_kw(kw::COLLATION)?;
                self.qualified_name()?;
            }
            return Ok(());
        }
        self.qualified_name().map(drop)
    }

    /// ( ' `<external collation name>` ' ) or ( ' `<external translation
    /// name>` ' ), after EXTERNAL [§11.30, §11.32]: a quoted name.
    fn external_name(&mut self) -> Result<()> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let Tok::CharacterString(_) = self.peek() else { return Err(self.syntax("expected a quoted name")) };
        self.advance();
        self.expect_delim(delim::RIGHT_PAREN)
    }

    /// [ISO/IEC 9075:1992 §11.28] `<character set definition>` ::= CREATE
    /// CHARACTER SET `<character set name>` \[AS\] GET `<existing character
    /// set name>` \[`<collate clause>` | COLLATION FROM `<collation
    /// source>`\]; [§11.30] `<collation definition>` ::= CREATE COLLATION
    /// `<collation name>` FOR `<character set specification>` FROM
    /// `<collation source>` \[NO PAD | PAD SPACE\]; [§11.32] `<translation
    /// definition>` ::= CREATE TRANSLATION `<translation name>` FOR
    /// `<character set specification>` TO `<character set specification>`
    /// FROM {EXTERNAL ( ' `<external translation name>` ' ) | IDENTITY |
    /// `<translation name>`}: after CREATE, if one is next, read, then
    /// refused (SQLCODE -900) at the statement's end; whether one was.
    pub(super) fn character_set_definition(&mut self) -> Result<bool> {
        let at = self.here();
        let what = if self.is_kw(kw::CHARACTER) && self.next_is_kw(kw::SET) {
            self.advance();
            self.advance();
            self.character_set_body()?
        } else if self.accept_kw(kw::COLLATION) {
            self.collation_body()?
        } else if self.accept_kw(kw::TRANSLATION) {
            self.translation_body()?
        } else {
            return Ok(false);
        };
        self.refuse_later(never(at, &what));
        Ok(true)
    }

    /// [§11.28] A `<character set definition>`, after CREATE CHARACTER SET:
    /// what it is.
    fn character_set_body(&mut self) -> Result<String> {
        let name = self.character_set_name()?;
        self.accept_kw(kw::AS);
        self.expect_kw(kw::GET)?;
        self.character_set_name()?;
        if self.accept_kw(kw::COLLATE) {
            self.qualified_name()?;
        } else if self.accept_kw(kw::COLLATION) {
            self.expect_kw(kw::FROM)?;
            self.collation_source()?;
        }
        Ok(format!("CREATE CHARACTER SET {name}"))
    }

    /// [§11.30] A `<collation definition>`, after CREATE COLLATION: what it
    /// is.
    fn collation_body(&mut self) -> Result<String> {
        let name = self.qualified_name()?;
        self.expect_kw(kw::FOR)?;
        self.character_set_name()?;
        self.expect_kw(kw::FROM)?;
        self.collation_source()?;
        if self.accept_kw(kw::NO) {
            self.expect_kw(kw::PAD)?;
        } else if self.accept_kw(kw::PAD) {
            self.expect_kw(kw::SPACE)?;
        }
        Ok(format!("CREATE COLLATION {name}"))
    }

    /// [§11.32] A `<translation definition>`, after CREATE TRANSLATION: what
    /// it is.
    fn translation_body(&mut self) -> Result<String> {
        let name = self.qualified_name()?;
        self.expect_kw(kw::FOR)?;
        self.character_set_name()?;
        self.expect_kw(kw::TO)?;
        self.character_set_name()?;
        self.expect_kw(kw::FROM)?;
        if self.accept_kw(kw::EXTERNAL) {
            self.external_name()?;
        } else if !self.accept_kw(kw::IDENTITY) {
            self.qualified_name()?;
        }
        Ok(format!("CREATE TRANSLATION {name}"))
    }

    /// [ISO/IEC 9075:1992 §11.29, §11.31, §11.33] DROP CHARACTER SET
    /// `<character set name>`, DROP COLLATION `<collation name>` or DROP
    /// TRANSLATION `<translation name>`, after DROP, if one is next: read,
    /// then refused (SQLCODE -900) at the statement's end.
    pub(super) fn drop_character_set(&mut self) -> Result<Option<SqlStatement>> {
        let at = self.here();
        let what = if self.is_kw(kw::CHARACTER) && self.next_is_kw(kw::SET) {
            self.advance();
            self.advance();
            format!("DROP CHARACTER SET {}", self.character_set_name()?)
        } else if self.accept_kw(kw::COLLATION) {
            format!("DROP COLLATION {}", self.qualified_name()?)
        } else if self.accept_kw(kw::TRANSLATION) {
            format!("DROP TRANSLATION {}", self.qualified_name()?)
        } else {
            return Ok(None);
        };
        let refused = never(at, &what);
        self.refuse_later(refused);
        Ok(Some(SqlStatement::Commit))
    }

    /// [ISO/IEC 9075:1992 §11.36] `<object name>` of COLLATION `<collation
    /// name>`, CHARACTER SET `<character set name>` or TRANSLATION
    /// `<translation name>`, after ON, if one is next: read, the privilege on
    /// it refused at the statement's end (SQLCODE -900), but USAGE on
    /// SQL_TEXT.
    pub(super) fn character_set_object(&mut self) -> Result<bool> {
        let at = self.here();
        if self.is_kw(kw::CHARACTER) && self.next_is_kw(kw::SET) {
            self.advance();
            self.advance();
            let name = self.character_set_name()?;
            let refused = never(at, &format!("a privilege on the character set {name}"));
            self.refuse_later(refused);
            return Ok(true);
        }
        for (word, what) in [(kw::COLLATION, "collation"), (kw::TRANSLATION, "translation")] {
            if self.accept_kw(word) {
                let name = self.qualified_name()?;
                let refused = never(at, &format!("a privilege on the {what} {name}"));
                self.refuse_later(refused);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// [ISO/IEC 9075:1992 §6.7] `<form-of-use conversion>` ::= CONVERT (
    /// `<character value expression>` USING `<form-of-use conversion name>`
    /// ), `<character translation>` ::= TRANSLATE ( `<character value
    /// expression>` USING `<translation name>` ), each name a `<qualified
    /// name>`: read, then refused (SQLCODE -900) at the statement's end;
    /// the expression converted stands for it.
    pub(super) fn conversion(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let Tok::KeyWord(word) = *self.peek() else { return Err(self.syntax("expected CONVERT or TRANSLATE")) };
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let converted = self.value_expression()?;
        self.expect_kw(kw::USING)?;
        let name = self.qualified_name()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let refused = never(at, &format!("{word} ... USING {name}"));
        self.refuse_later(refused);
        Ok(converted)
    }

    /// [ISO/IEC 9075:1992 §16.3] `<set names statement>` ::= SET NAMES
    /// `<value specification>`, after SET: read, then refused (SQLCODE
    /// -900): there is one character set, and its names are of it.
    pub(super) fn set_names(&mut self) -> Result<SqlStatement> {
        let at = self.here();
        self.expect_kw(kw::NAMES)?;
        self.value_specification()?;
        let refused = never(at, "SET NAMES");
        self.refuse_later(refused);
        Ok(SqlStatement::Commit)
    }

    /// [ISO/IEC 9075:1992 §5.3, §5.4] An `<introducer>` `<character set
    /// specification>`, if next, before a literal or an identifier: taken
    /// if it names SQL_TEXT, else refused once the statement is read.
    pub(super) fn introducer(&mut self) {
        let at = self.here();
        if let Tok::Introducer(name) = self.peek().clone() {
            self.advance();
            self.only_sql_text(at, &name);
        }
    }
}
