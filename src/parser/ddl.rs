//! \[EXTENSION\] SQL-schema statements of ISO/IEC 9075:1992 run as SQL
//! statements, in a transaction: `<alter table statement>` [§11.10],
//! `<drop schema statement>` [§11.2], `<drop table statement>` [§11.18],
//! `<drop view statement>` [§11.20], `<drop domain statement>` [§11.27]
//! and `<drop assertion statement>` [§11.35]; and the index statements and
//! table storage clauses of this implementation (RULES.md D41, D48), in
//! the standard's notation:
//!
//! ```text
//! <alter table storage> ::=
//!       SET <table options>
//!     | USING <engine name> [ WITH <table options> ]
//!     | RESEAL
//! ```
//!
//! An `<alter table storage>` is an `<alter table action>`. RESEAL is a
//! `<non-reserved word>`, an identifier elsewhere: the table's storage made
//! anew as it is, its rows sealed again through the encodings it names
//! now. Split from parser.rs (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{AlterAction, DropBehavior, DropObject, IndexStatement, OptionValue, SqlStatement, TableOptionValue, TableStorage};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// \[EXTENSION\] The `<non-reserved word>` of `<alter table storage>` that
/// seals a table's rows again (RULES.md D48).
pub const RESEAL: &str = "RESEAL";

impl Parser {
    /// An SQL-schema statement: CREATE TABLE, VIEW, DOMAIN or ASSERTION,
    /// CREATE GLOBAL or LOCAL TEMPORARY TABLE, or ALTER DOMAIN
    /// (`parser/schema.rs`); ALTER TABLE; DROP SCHEMA, TABLE, VIEW, DOMAIN or
    /// ASSERTION; CREATE or DROP of a character set, collation or translation,
    /// read then refused (`parser/charsets.rs`); or, of this implementation,
    /// CREATE, DROP or ALTER INDEX, CREATE, ALTER or DROP USER, and ALTER
    /// DATABASE (`parser/users.rs`).
    pub(super) fn schema_manipulation(&mut self) -> Result<SqlStatement> {
        let defines = [kw::TABLE, kw::VIEW, kw::GLOBAL, kw::LOCAL, kw::DOMAIN, kw::ASSERTION].iter().any(|w| self.next_is_kw(w));
        if (self.is_kw(kw::CREATE) && defines) || (self.is_kw(kw::ALTER) && self.next_is_kw(kw::DOMAIN)) {
            return self.schema_statement();
        }
        if self.accept_kw(kw::CREATE) {
            if self.accept_kw(kw::USER) {
                return self.user_definition();
            }
            if self.character_set_definition()? {
                return Ok(SqlStatement::Commit);
            }
            return self.create_index();
        }
        let alter = self.accept_kw(kw::ALTER);
        if !alter {
            self.expect_kw(kw::DROP)?;
        }
        if self.accept_kw(kw::USER) {
            return self.alter_or_drop_user(alter);
        }
        if alter && self.accept_word(super::users::DATABASE) {
            return self.alter_database();
        }
        if !alter && let Some(refused) = self.drop_character_set()? {
            return Ok(refused);
        }
        if self.accept_word("INDEX") {
            let at = self.here();
            let name = (self.identifier()?, at);
            if !alter {
                return Ok(SqlStatement::Index(IndexStatement::Drop { name }));
            }
            self.expect_kw(kw::SET)?;
            return Ok(SqlStatement::Index(IndexStatement::Alter { name, options: self.table_options()? }));
        }
        if alter { self.alter_statement() } else { self.drop_statement() }
    }

    /// \[EXTENSION\] `<create index statement>` ::= CREATE INDEX `<index
    /// name>` ON `<table name>` ( `<column name>` \[{, `<column name>`}...\] )
    /// \[USING `<method name>`\] \[WITH `<table options>`\], after CREATE.
    fn create_index(&mut self) -> Result<SqlStatement> {
        if !self.accept_word("INDEX") {
            return Err(self.syntax("expected INDEX: a <schema> is processed by Database::schema"));
        }
        let named_at = self.here();
        let name = (self.identifier()?, named_at);
        self.expect_kw(kw::ON)?;
        let table = self.table_name()?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut columns = Vec::new();
        loop {
            let at = self.here();
            columns.push((self.identifier()?, at));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        let mut storage = TableStorage::default();
        if self.accept_kw(kw::USING) {
            let at = self.here();
            storage.engine = Some((self.identifier()?, at));
        }
        if self.accept_kw(kw::WITH) {
            storage.options = self.table_options()?;
        }
        Ok(SqlStatement::Index(IndexStatement::Create { name, table, columns, storage }))
    }

    /// [ISO/IEC 9075:1992 §11.2, §11.18, §11.20] `<drop schema statement>`
    /// ::= DROP SCHEMA `<schema name>` `<drop behavior>`; `<drop table
    /// statement>` ::= DROP TABLE `<table name>` `<drop behavior>`; `<drop
    /// view statement>` ::= DROP VIEW `<table name>` `<drop behavior>`;
    /// [§11.27] `<drop domain statement>` ::= DROP DOMAIN `<domain name>`
    /// `<drop behavior>`.
    fn drop_statement(&mut self) -> Result<SqlStatement> {
        // [§11.35] DROP ASSERTION <constraint name>: no <drop behavior>.
        if self.accept_kw(kw::ASSERTION) {
            return Ok(SqlStatement::Drop { object: DropObject::Assertion(self.table_name()?), behavior: DropBehavior::Restrict });
        }
        let object = if self.accept_kw(kw::SCHEMA) {
            let (catalog, name, at) = self.schema_name()?;
            DropObject::Schema { catalog, name, at }
        } else if self.accept_kw(kw::DOMAIN) {
            DropObject::Domain(self.table_name()?)
        } else if self.accept_kw(kw::VIEW) {
            DropObject::View(self.table_name()?)
        } else {
            self.expect_kw(kw::TABLE)?;
            DropObject::Table(self.table_name()?)
        };
        Ok(SqlStatement::Drop { object, behavior: self.drop_behavior()? })
    }

    /// [ISO/IEC 9075:1992 §11.10] `<alter table statement>` ::= ALTER TABLE
    /// `<table name>` `<alter table action>`
    fn alter_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::TABLE)?;
        let table = self.table_name()?;
        let start = self.here();
        let action = self.alter_table_action()?;
        Ok(SqlStatement::AlterTable { table, action, text: self.text_from(start) })
    }

    /// [ISO/IEC 9075:1992 §11.10] `<alter table action>`, alone: as the log
    /// keeps it (`Change::AlterTable`).
    ///
    /// # Errors
    ///
    /// The text is not an `<alter table action>` of those implemented.
    pub fn alter_table_action(&mut self) -> Result<AlterAction> {
        // [EXTENSION] SET <table options> | USING <engine name> [WITH
        // <table options>] | RESEAL: no engine and no option.
        if self.accept_word(RESEAL) {
            return Ok(AlterAction::Storage(TableStorage::default()));
        }
        if self.accept_kw(kw::SET) {
            return Ok(AlterAction::Storage(TableStorage { engine: None, options: self.table_options()? }));
        }
        if self.accept_kw(kw::USING) {
            let at = self.here();
            let engine = Some((self.identifier()?, at));
            let options = if self.accept_kw(kw::WITH) { self.table_options()? } else { Vec::new() };
            return Ok(AlterAction::Storage(TableStorage { engine, options }));
        }
        if self.accept_kw(kw::ADD) {
            // [§11.16] ADD <table constraint definition>, or [§11.11] ADD
            // [COLUMN] <column definition>.
            if self.at_table_constraint() {
                return Ok(AlterAction::AddConstraint(self.table_constraint()?));
            }
            self.accept_kw(kw::COLUMN);
            return Ok(AlterAction::AddColumn(self.column_definition()?));
        }
        if self.accept_kw(kw::ALTER) {
            // [§11.12] ALTER [COLUMN] <column name> {SET <default clause> | DROP DEFAULT}
            self.accept_kw(kw::COLUMN);
            let at = self.here();
            let column = self.identifier()?;
            let default = if self.accept_kw(kw::SET) {
                self.expect_kw(kw::DEFAULT)?;
                Some(self.default_option()?)
            } else {
                self.expect_kw(kw::DROP)?;
                self.expect_kw(kw::DEFAULT)?;
                None
            };
            return Ok(AlterAction::AlterColumn { column, at, default });
        }
        self.expect_kw(kw::DROP)?;
        if self.accept_kw(kw::CONSTRAINT) {
            // [§11.17] DROP CONSTRAINT <constraint name> <drop behavior>
            let name = self.table_name()?;
            return Ok(AlterAction::DropConstraint { name, behavior: self.drop_behavior()? });
        }
        // [§11.15] DROP [COLUMN] <column name> <drop behavior>
        self.accept_kw(kw::COLUMN);
        let at = self.here();
        let column = self.identifier()?;
        Ok(AlterAction::DropColumn { column, at, behavior: self.drop_behavior()? })
    }

    /// \[EXTENSION\] `<table options>` ::= ( `<table option>` \[{, `<table
    /// option>`}...\] ); `<table option>` ::= `<option name>` =
    /// {`<literal>` | `<identifier>` | DEFAULT} (RULES.md D41).
    pub(super) fn table_options(&mut self) -> Result<Vec<TableOptionValue>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut options = Vec::new();
        loop {
            let at = self.here();
            let mut parts = vec![self.option_word()?];
            while self.accept_delim(delim::PERIOD) {
                parts.push(self.option_word()?);
            }
            self.expect_delim(delim::EQUALS)?;
            let value = if self.accept_kw(kw::DEFAULT) { None } else { Some(self.option_value()?) };
            options.push(TableOptionValue { name: parts.join("."), at, value });
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(options)
    }

    /// A part of an `<option name>`: an `<identifier>`, or a `<key word>`,
    /// as options are named by their plug-ins, not by SQL.
    pub(super) fn option_word(&mut self) -> Result<String> {
        if let Tok::KeyWord(k) = self.peek() {
            let word = (*k).to_owned();
            self.advance();
            return Ok(word);
        }
        self.identifier()
    }

    /// The value of a `<table option>`: a `<literal>`, or an
    /// `<identifier>` or `<key word>` as a word.
    fn option_value(&mut self) -> Result<OptionValue> {
        let word = match self.peek().clone() {
            Tok::Identifier(_) => return Ok(OptionValue::Word(self.identifier()?)),
            Tok::KeyWord(k) => k,
            Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Introducer(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::Delimiter(_)
            | Tok::End => {
                return Ok(OptionValue::Literal(self.literal()?));
            }
        };
        self.advance();
        Ok(OptionValue::Word(word.to_owned()))
    }

    /// [ISO/IEC 9075:1992 §11.2] `<drop behavior>` ::= CASCADE | RESTRICT
    pub(super) fn drop_behavior(&mut self) -> Result<DropBehavior> {
        if self.accept_kw(kw::CASCADE) {
            return Ok(DropBehavior::Cascade);
        }
        if self.accept_kw(kw::RESTRICT) {
            return Ok(DropBehavior::Restrict);
        }
        Err(self.syntax("expected CASCADE or RESTRICT"))
    }
}
