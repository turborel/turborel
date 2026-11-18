//! \[EXTENSION\] SQL-schema statements of ISO/IEC 9075:1992 run as SQL
//! statements, in a transaction: `<alter table statement>` [§11.10],
//! `<drop schema statement>` [§11.2], `<drop table statement>` [§11.18],
//! `<drop view statement>` [§11.20], `<drop domain statement>` [§11.27]
//! and `<drop assertion statement>` [§11.35]; ISO/IEC 9075-2:2023's
//! `<trigger definition>` [§11.49] and `<drop trigger statement>` [§11.50]
//! (`parser/triggers.rs`); and the index statements and
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
//! now.

mod hybrid_index;
mod vector_index;

pub use vector_index::{method_of_type, option_of};

use super::Parser;
use crate::ast::{
    AlterAction, AlterColumnAction, DropBehavior, DropObject, IndexStatement, OptionValue, SqlStatement, TableOptionValue, TableStorage,
};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// \[EXTENSION\] The `<non-reserved word>` of `<alter table storage>` that
/// seals a table's rows again (RULES.md D48).
pub const RESEAL: &str = "RESEAL";

/// \[EXTENSION\] The `<non-reserved word>` that begins RENAME \[COLUMN\]
/// `<column name>` TO `<column name>` (`db/alter/rename.rs`).
const RENAME: &str = "RENAME";

/// The key words after CREATE that begin a definition `schema_statement`
/// reads.
const DEFINED: [&str; 7] = [kw::TABLE, kw::VIEW, kw::GLOBAL, kw::LOCAL, kw::DOMAIN, kw::ASSERTION, kw::COLLATION];

/// What the log writes for columns of a collation dropped.
pub const REVERT_COLLATION: &str = "REVERT_COLLATION";

impl Parser {
    /// What a DROP of a collation, a sequence generator, a routine or a
    /// type names, its `<drop behavior>` next, if one follows DROP here:
    /// COLLATION, then its name; [ISO/IEC 9075-2:2023 §11.74] SEQUENCE,
    /// then a `<sequence generator name>`; [§11.62] a `<specific routine
    /// designator>`; [§11.59] TYPE, then a `<schema-resolved user-defined
    /// type name>`.
    ///
    /// # Errors
    ///
    /// The text after the word is not the name it takes.
    fn dropped_object(&mut self) -> Result<Option<DropObject>> {
        if self.accept_kw(kw::COLLATION) {
            return Ok(Some(DropObject::Collation(self.table_name()?)));
        }
        if self.accept_word(super::sequences::SEQUENCE) {
            return Ok(Some(DropObject::Sequence(self.table_name()?)));
        }
        if self.next_is_routine_designator() {
            return Ok(Some(DropObject::Routine(self.routine_designator()?)));
        }
        if self.accept_word(super::user_types::TYPE) {
            return Ok(Some(DropObject::Type(self.table_name()?)));
        }
        // [ISO/IEC 9075-2:2023 §11.64, §11.66, §11.71] CAST, ORDERING,
        // TRANSFORM.
        if let Some(object) = self.dropped_of_type()? {
            return Ok(Some(object));
        }
        // [ISO/IEC 9075-4:2006 draft §9.22] MODULE <SQL-server module name>.
        if self.is_kw(kw::MODULE) && !matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD)) {
            self.advance();
            return Ok(Some(DropObject::Module(self.table_name()?)));
        }
        Ok(None)
    }

    /// An SQL-schema statement: CREATE TABLE, VIEW, DOMAIN, ASSERTION,
    /// COLLATION, SEQUENCE or TRIGGER, CREATE GLOBAL or LOCAL TEMPORARY TABLE, or
    /// ALTER DOMAIN or SEQUENCE (`parser/schema.rs`); ALTER TABLE; DROP
    /// SCHEMA, TABLE, VIEW, DOMAIN, ASSERTION, COLLATION, SEQUENCE or TRIGGER; CREATE
    /// or DROP of a character set or translation,
    /// read then refused (`parser/charsets.rs`); or, of this implementation,
    /// CREATE, DROP or ALTER INDEX, CREATE, ALTER or DROP USER, and ALTER
    /// DATABASE (`parser/users.rs`).
    pub(super) fn schema_manipulation(&mut self) -> Result<SqlStatement> {
        if self.at_role_definition() {
            return self.schema_statement();
        }
        let other = self.next_is_module() || self.next_is_of_type();
        let defines = DEFINED.iter().any(|w| self.next_is_kw(w)) || self.next_is_trigger() || self.next_is_routine() || other;
        let altered = self.next_is_kw(kw::DOMAIN) || self.next_is_sequence() || self.next_is_type() || self.next_is_transform();
        if (self.is_kw(kw::CREATE) && (defines || self.next_is_sequence() || self.next_is_type())) || (self.is_kw(kw::ALTER) && altered) {
            return self.schema_statement();
        }
        if self.accept_kw(kw::CREATE) {
            if self.accept_kw(kw::USER) {
                return self.user_definition();
            }
            if self.character_set_definition()? {
                return Ok(SqlStatement::Commit);
            }
            return self.create_any_index();
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
        if !alter && let Some(object) = self.dropped_object()? {
            return Ok(SqlStatement::Drop { object, behavior: self.drop_behavior()? });
        }
        if self.accept_word("INDEX") {
            let at = self.here();
            let name = (self.identifier()?, at);
            if !alter {
                return Ok(SqlStatement::Index(IndexStatement::Drop { name }));
            }
            if let Some(action) = self.index_maintenance()? {
                return Ok(SqlStatement::Index(IndexStatement::Maintain { name, action }));
            }
            self.expect_kw(kw::SET)?;
            return Ok(SqlStatement::Index(IndexStatement::Alter { name, options: self.table_options()? }));
        }
        if alter { self.alter_statement() } else { self.drop_statement() }
    }

    /// \[EXTENSION\] `<create index statement>` ::= CREATE INDEX `<index
    /// name>` ON `<table name>` ( `<column name>` \[{, `<column name>`}...\] )
    /// \[USING `<method name>`\] \[WITH `<table options>`\], after CREATE;
    /// CREATE VECTOR INDEX is `ddl/vector_index.rs`, CREATE HYBRID VECTOR
    /// INDEX `ddl/hybrid_index.rs`.
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
            let given = self.identifier()?;
            // [EXTENSION] SPARSE is the other name of the method `inverted`.
            let method = if given.eq_ignore_ascii_case(vector_index::SPARSE) { vector_index::INVERTED.to_owned() } else { given };
            storage.engine = Some((method, at));
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
        if let Some(drop) = self.drop_trigger()? {
            return Ok(drop);
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

    /// [ISO/IEC 9075-2:2023 §11.27] ADD <table period definition>, ADD
    /// written: the period, and the columns it adds, of SYSTEM_TIME.
    ///
    /// # Errors
    ///
    /// The text is no period, or a column of it no `<column definition>`.
    fn added_period(&mut self) -> Result<AlterAction> {
        let period = self.period_definition()?;
        // [§11.27] <add system time period column list>.
        let mut columns = Vec::new();
        while columns.len() < 2 && self.accept_kw(kw::ADD) {
            self.accept_kw(kw::COLUMN);
            columns.push(self.column_definition()?);
        }
        Ok(AlterAction::AddPeriod(period, columns))
    }

    /// [ISO/IEC 9075-2:2023 §11.29, §11.30] ADD SYSTEM VERSIONING, or DROP
    /// SYSTEM VERSIONING <drop behavior>, if the text is at one.
    ///
    /// # Errors
    ///
    /// DROP SYSTEM VERSIONING is of no `<drop behavior>`.
    fn versioning_action(&mut self) -> Result<Option<AlterAction>> {
        let (add, drop) = (self.at_versioning(kw::ADD), self.at_versioning(kw::DROP));
        if !add && !drop {
            return Ok(None);
        }
        let at = self.here();
        for _ in 0_u8..3 {
            self.advance();
        }
        Ok(Some(if add { AlterAction::AddVersioning(at) } else { AlterAction::DropVersioning { at, behavior: self.drop_behavior()? } }))
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
        // Of the log alone: no statement writes it (`AlterAction`).
        if matches!(self.catalogs, super::Catalogs::Kept) && self.accept_word(REVERT_COLLATION) {
            let at = self.table_name()?;
            let auth = at.auth.ok_or_else(|| self.syntax("a collation reverted names its schema"))?;
            return Ok(AlterAction::RevertCollation(QualifiedName { auth, ident: at.ident }));
        }
        // Of the log alone too: as TRUNCATE TABLE writes it.
        if matches!(self.catalogs, super::Catalogs::Kept) && self.accept_word(super::TRUNCATE) {
            let restart = self.accept_word(super::generated::RESTART);
            if restart {
                self.expect_kw(kw::IDENTITY)?;
            }
            return Ok(AlterAction::Truncate { restart });
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
        if let Some(versioning) = self.versioning_action()? {
            return Ok(versioning);
        }
        if self.accept_kw(kw::ADD) {
            // [§11.16] ADD <table constraint definition>, or [§11.11] ADD
            // [COLUMN] <column definition>.
            if self.at_table_constraint() {
                return Ok(AlterAction::AddConstraint(self.table_constraint()?));
            }
            if self.at_period() {
                return self.added_period();
            }
            self.accept_kw(kw::COLUMN);
            return Ok(AlterAction::AddColumn(self.column_definition()?));
        }
        if self.accept_kw(kw::ALTER) {
            // [§11.12] ALTER [COLUMN] <column name> <alter column action>
            self.accept_kw(kw::COLUMN);
            let at = self.here();
            let column = self.identifier()?;
            return Ok(AlterAction::AlterColumn { column, at, action: self.alter_column_action()? });
        }
        if let Some(rename) = self.rename_action()? {
            return Ok(rename);
        }
        self.expect_kw(kw::DROP)?;
        self.drop_action()
    }

    /// \[EXTENSION\] RENAME \[COLUMN\] `<column name>` TO `<column
    /// name>`, if RENAME begins it (`db/alter/rename.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it is not written so.
    fn rename_action(&mut self) -> Result<Option<AlterAction>> {
        if !self.accept_word(RENAME) {
            return Ok(None);
        }
        self.accept_kw(kw::COLUMN);
        let at = self.here();
        let column = self.identifier()?;
        self.expect_kw(kw::TO)?;
        Ok(Some(AlterAction::AlterColumn { column, at, action: AlterColumnAction::Rename(self.identifier()?) }))
    }

    /// The `<alter table action>` that DROP begins, after DROP.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it is not written so.
    fn drop_action(&mut self) -> Result<AlterAction> {
        // [ISO/IEC 9075-2:2023 §11.28] DROP <system or application time
        // period specification> <drop behavior>.
        if self.at_period() {
            let at = self.here();
            let name = self.period_name()?;
            return Ok(AlterAction::DropPeriod { name, at, behavior: self.drop_behavior()? });
        }
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
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
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
