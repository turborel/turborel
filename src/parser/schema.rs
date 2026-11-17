//! The schema grammar: `<schema>` [X3.135-1986 §6.1], its `<table
//! definition>`s [X3.135-1986 §6.2-§6.4], `<view definition>`s [X3.135-1986
//! §6.5] and `<privilege definition>`s [X3.135-1986 §6.6]; temporary tables
//! [ISO/IEC 9075:1992 §11.3, §13.11] and the `<revoke statement>` [§11.37].
//! Domains and assertions are in `parser/domains.rs`, triggers in
//! `parser/triggers.rs`.

use super::Parser;
use crate::ast::{
    CheckLevels, CollationDefinition, ColumnConstraint, ColumnConstraintKind, ColumnDefinition, ColumnGeneration, ConstraintAttributes,
    DefaultOption, QueryExpression, Schema, SchemaStatement, SqlStatement, TableDefinition, TableElement, TableName, TableStorage,
    Temporary, TemporaryKind, TypeName, ViewDefinition,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// [X3.135-1986 §6.1; ISO/IEC 9075:1992 §11.1] `<schema definition>`
    /// ::= CREATE SCHEMA `<schema name clause>` \[`<schema element>`...\]
    ///
    /// # Errors
    ///
    /// The text is not a `<schema>`, or uses a part of one not implemented.
    pub fn schema(&mut self) -> Result<Schema> {
        self.expect_kw(kw::CREATE)?;
        self.expect_kw(kw::SCHEMA)?;
        let (catalog, name, auth) = self.schema_name_clause()?;
        self.schema_character_set()?;

        let mut schema = Schema {
            catalog,
            name,
            auth,
            collations: Vec::new(),
            sequences: Vec::new(),
            types: Vec::new(),
            domains: Vec::new(),
            assertions: Vec::new(),
            tables: Vec::new(),
            views: Vec::new(),
            privileges: Vec::new(),
            roles: Vec::new(),
            triggers: Vec::new(),
            routines: Vec::new(),
            modules: Vec::new(),
            defined: Vec::new(),
        };
        while self.schema_element(&mut schema)? {}
        self.end()?;
        Ok(schema)
    }

    /// A `<schema element>` of `s`, taken into it, if one is next: whether
    /// it was.
    ///
    /// `<schema element>` ::= `<domain definition>` | `<table definition>`
    /// | `<view definition>` | `<grant statement>` | `<assertion
    /// definition>` | `<character set definition>` | `<collation
    /// definition>` | `<translation definition>` | `<trigger definition>` |
    /// `<schema routine>` | `<sequence generator definition>` |
    /// `<user-defined type definition>` | `<user-defined cast definition>`
    /// | `<user-defined ordering definition>` | `<transform definition>` |
    /// `<role definition>` [ISO/IEC 9075-2:2023 §11.1], a `<grant
    /// statement>` a `<grant privilege statement>` or a `<grant role
    /// statement>` [§12.1], a `<schema routine>` of a method too.
    fn schema_element(&mut self, s: &mut Schema) -> Result<bool> {
        if self.is_kw(kw::GRANT) && self.at_role_grant() {
            s.roles.push(self.grant_role()?);
        } else if self.is_kw(kw::GRANT) {
            s.privileges.push(self.privilege_definition()?);
        } else if self.is_kw(kw::CREATE) && self.at_role_definition() {
            s.roles.push(self.role_definition()?);
        } else if !self.is_kw(kw::CREATE) {
            return Ok(false);
        } else if self.character_set_element(&mut s.collations)? {
            return Ok(true);
        } else if self.next_is_sequence() {
            s.sequences.push(self.sequence_definition()?);
        } else if self.next_is_type() {
            s.types.push(self.type_definition()?);
        } else if matches!(self.peek_at(1), Tok::KeyWord(kw::VIEW)) {
            s.views.push(self.view_definition()?);
        } else if matches!(self.peek_at(1), Tok::KeyWord(kw::DOMAIN)) {
            s.domains.push(self.domain_definition()?);
        } else if matches!(self.peek_at(1), Tok::KeyWord(kw::ASSERTION)) {
            s.assertions.push(self.assertion_definition()?);
        } else if self.next_is_trigger() {
            s.triggers.push(self.trigger_definition()?);
        } else if self.next_is_routine() {
            s.routines.push(self.routine_definition()?);
        } else if self.next_is_of_type() {
            let before = s.routines.len();
            let Some(defined) = self.defined_for_type()? else { return Err(self.syntax("expected a method, cast, ordering or transform")) };
            s.defined.push((before, defined));
        } else if self.next_is_module() {
            let schema = s.name.clone();
            s.modules.push(self.module_definition(Some(&schema))?);
        } else {
            s.tables.push(self.table_definition()?);
        }
        Ok(true)
    }

    /// [§11.1] `<schema name clause>` ::= `<schema name>` | AUTHORIZATION
    /// `<schema authorization identifier>` | `<schema name>` AUTHORIZATION
    /// `<schema authorization identifier>`: the catalog's name, if written,
    /// the schema's, the `<schema authorization identifier>` if written
    /// and, without a `<schema name>`, the schema's name too \[SR1\].
    ///
    /// # Errors
    ///
    /// The text is not one.
    fn schema_name_clause(&mut self) -> Result<(Option<String>, String, Option<String>)> {
        let named = if self.is_kw(kw::AUTHORIZATION) { None } else { Some(self.schema_name()?) };
        let auth = if self.accept_kw(kw::AUTHORIZATION) { Some(self.identifier()?) } else { None };
        match (named, auth) {
            (Some((catalog, name, _)), auth) => Ok((catalog, name, auth)),
            (None, Some(auth)) => Ok((None, auth.clone(), Some(auth))),
            (None, None) => Err(self.syntax("expected a <schema name> or AUTHORIZATION")),
        }
    }

    /// [§11.1] `<schema character set specification>` ::= DEFAULT
    /// CHARACTER SET `<character set specification>`, if written. SQL_TEXT's
    /// is taken.
    fn schema_character_set(&mut self) -> Result<()> {
        if !self.accept_kw(kw::DEFAULT) {
            return Ok(());
        }
        if !self.is_kw(kw::CHARACTER) {
            return Err(self.syntax("expected CHARACTER SET"));
        }
        self.character_set_clause()
    }

    /// [ISO/IEC 9075:1992 §5.4] `<schema name>` ::= \[`<catalog name>` .\]
    /// `<unqualified schema name>`: the catalog's name, if written, the
    /// schema's, and where it begins.
    pub(super) fn schema_name(&mut self) -> Result<(Option<String>, String, usize)> {
        let at = self.here();
        let first = self.identifier()?;
        if self.accept_delim(delim::PERIOD) {
            return Ok((Some(first), self.identifier()?, at));
        }
        Ok((None, first, at))
    }

    /// [§11.1] A character set, translation or collation definition, if
    /// next, at CREATE: a character set's or translation's read, then
    /// refused; a collation's added to `collations`. Whether one was.
    fn character_set_element(&mut self, collations: &mut Vec<CollationDefinition>) -> Result<bool> {
        if matches!(self.peek_at(1), Tok::KeyWord(kw::CHARACTER | kw::TRANSLATION)) {
            self.advance();
            self.character_set_definition()?;
            return Ok(true);
        }
        if !matches!(self.peek_at(1), Tok::KeyWord(kw::COLLATION)) {
            return Ok(false);
        }
        self.advance();
        self.advance();
        collations.push(self.collation_definition()?);
        Ok(true)
    }

    /// \[EXTENSION\] [ISO/IEC 9075:1992 §20.1] A `<table definition>`, a
    /// `<view definition>`, a `<domain definition>`, an `<alter domain
    /// statement>`, an `<assertion definition>`, a `<grant statement>`, a
    /// `<revoke statement>` or [ISO/IEC 9075-2:2023 §11.49] a `<trigger
    /// definition>`, on its own: with its text.
    pub(super) fn schema_statement(&mut self) -> Result<SqlStatement> {
        let start = self.here();
        let statement = if let Some(r) = self.role_statement()? {
            SchemaStatement::Role(r)
        } else if self.is_kw(kw::GRANT) {
            SchemaStatement::Grant(self.privilege_definition()?)
        } else if self.is_kw(kw::REVOKE) {
            SchemaStatement::Revoke(self.revoke_statement()?)
        } else if self.next_is_kw(kw::VIEW) {
            SchemaStatement::View(self.view_definition()?)
        } else if self.next_is_kw(kw::DOMAIN) && self.is_kw(kw::ALTER) {
            SchemaStatement::AlterDomain(self.alter_domain()?)
        } else if self.next_is_kw(kw::DOMAIN) {
            SchemaStatement::Domain(self.domain_definition()?)
        } else if self.next_is_kw(kw::ASSERTION) {
            SchemaStatement::Assertion(self.assertion_definition()?)
        } else if self.next_is_kw(kw::COLLATION) {
            self.advance();
            self.advance();
            SchemaStatement::Collation(self.collation_definition()?)
        } else if self.next_is_sequence() && self.is_kw(kw::ALTER) {
            SchemaStatement::AlterSequence(self.alter_sequence()?)
        } else if self.next_is_sequence() {
            SchemaStatement::Sequence(self.sequence_definition()?)
        } else if let Some(defined) = self.defined_for_type()? {
            defined
        } else if self.next_is_trigger() {
            SchemaStatement::Trigger(self.trigger_definition()?)
        } else if self.next_is_routine() {
            SchemaStatement::Routine(Box::new(self.routine_definition()?))
        } else if self.next_is_module() {
            SchemaStatement::Module(Box::new(self.module_definition(None)?))
        } else {
            SchemaStatement::Table(self.table_definition()?)
        };
        Ok(SqlStatement::Schema { statement, text: self.text_from(start) })
    }

    /// [X3.135-1986 §6.5; ISO/IEC 9075:1992 §11.19] `<view definition>` ::=
    /// CREATE VIEW `<table name>` \[(`<view column list>`)\] AS `<query
    /// expression>` \[WITH \[`<levels clause>`\] CHECK OPTION\]
    fn view_definition(&mut self) -> Result<ViewDefinition> {
        self.expect_kw(kw::CREATE)?;
        self.expect_kw(kw::VIEW)?;
        let name = self.table_name()?;
        // [ISO/IEC 9075-2:2023 §11.32] a <referenceable view specification>.
        let typed = if self.accept_kw(kw::OF) { Some(Box::new(self.referenceable_view()?)) } else { None };
        // <view column list> ::= <column name> [{,<column name>}...]
        let columns = if typed.is_none() && self.is_delim(delim::LEFT_PAREN) { Some(self.column_list()?) } else { None };
        self.expect_kw(kw::AS)?;
        let start = self.here();
        let query = self.query_expression()?;
        let text = self.text_from(start);
        let check_option = if self.accept_kw(kw::WITH) {
            // [ISO/IEC 9075:1992 §11.19 SR13] CASCADED unless LOCAL is written.
            let levels = if self.accept_kw(kw::LOCAL) {
                CheckLevels::Local
            } else {
                self.accept_kw(kw::CASCADED);
                CheckLevels::Cascaded
            };
            self.expect_kw(kw::CHECK)?;
            self.expect_kw(kw::OPTION)?;
            Some(levels)
        } else {
            None
        };
        Ok(ViewDefinition { name, columns, query, text, check_option, typed })
    }

    /// The `<query expression>` of a view, as the catalog keeps it
    /// [X3.135-1986 §6.5; ISO/IEC 9075:1992 §11.19].
    ///
    /// # Errors
    ///
    /// The text is not a `<query expression>`.
    pub fn view_query(&mut self) -> Result<QueryExpression> {
        let query = self.query_expression()?;
        self.end()?;
        Ok(query)
    }

    /// [X3.135-1986 §6.2] `<table definition>` ::= CREATE TABLE `<table name>`
    ///                            (`<table element>` \[{,`<table element>`}...\]),
    /// or of ISO/IEC 9075-2:2023 §11.3 with an `<as subquery clause>`.
    fn table_definition(&mut self) -> Result<TableDefinition> {
        self.expect_kw(kw::CREATE)?;
        // [ISO/IEC 9075:1992 §11.3] [{GLOBAL | LOCAL} TEMPORARY]
        let kind = if self.accept_kw(kw::GLOBAL) {
            Some(TemporaryKind::Global)
        } else if self.accept_kw(kw::LOCAL) {
            Some(TemporaryKind::CreatedLocal)
        } else {
            None
        };
        if kind.is_some() {
            self.expect_kw(kw::TEMPORARY)?;
        }
        self.expect_kw(kw::TABLE)?;
        let name = self.table_name()?;
        // [ISO/IEC 9075-2:2023 §11.3] a <typed table clause>.
        let (elements, subquery, typed) = if self.accept_kw(kw::OF) {
            let (typed, elements) = self.typed_table_clause(kind.is_some())?;
            (elements, None, Some(typed))
        } else {
            let (elements, subquery) = self.table_contents()?;
            (elements, subquery, None)
        };
        // [ISO/IEC 9075-2:2023 §11.3] WITH <system versioning clause>.
        let versioning = self.at_versioning(kw::WITH).then(|| self.here());
        if versioning.is_some() {
            self.advance();
            self.advance();
            self.advance();
        }
        let on_commit = self.on_commit()?;
        // [§11.3 SR3] ON COMMIT only of a temporary table; [SR4] DELETE
        // ROWS if not written.
        let temporary = match (kind, on_commit) {
            (Some(kind), delete_rows) => Some(Temporary { kind, delete_rows: delete_rows.unwrap_or(true) }),
            (None, Some(_)) => return Err(self.rule("[ISO/IEC 9075:1992 §11.3 SR3] ON COMMIT is of a TEMPORARY table")),
            (None, None) => None,
        };
        let mut storage = TableStorage::default();
        if self.accept_kw(kw::USING) {
            storage.engine = Some((self.identifier()?, self.here()));
        }
        if self.accept_kw(kw::WITH) {
            storage.options = self.table_options()?;
        }
        Ok(TableDefinition { name, temporary, elements, subquery, storage, typed, versioning })
    }

    /// [ISO/IEC 9075:1992 §13.11] `<temporary table declaration>` ::=
    /// DECLARE LOCAL TEMPORARY TABLE `<qualified local table name>` `<table
    /// element list>` \[ON COMMIT {PRESERVE | DELETE} ROWS\], DELETE ROWS if
    /// none is written \[SR5\].
    pub(super) fn temporary_table_declaration(&mut self) -> Result<TableDefinition> {
        self.expect_kw(kw::DECLARE)?;
        self.expect_kw(kw::LOCAL)?;
        self.expect_kw(kw::TEMPORARY)?;
        self.expect_kw(kw::TABLE)?;
        let at = self.here();
        self.expect_kw(kw::MODULE)?;
        self.expect_delim(delim::PERIOD)?;
        let name = TableName { auth: Some(self.module_qualifier()), ident: self.identifier()?, at };
        let elements = self.table_element_list()?;
        let delete_rows = self.on_commit()?.unwrap_or(true);
        let temporary = Some(Temporary { kind: TemporaryKind::Declared, delete_rows });
        Ok(TableDefinition { name, temporary, elements, subquery: None, storage: TableStorage::default(), typed: None, versioning: None })
    }

    /// [ISO/IEC 9075:1992 §11.3] `<table element list>` ::= ( `<table
    /// element>` \[{, `<table element>`}...\] )
    pub(super) fn table_element_list(&mut self) -> Result<Vec<TableElement>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut elements = vec![self.table_element()?];
        while self.accept_delim(delim::COMMA) {
            elements.push(self.table_element()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(elements)
    }

    /// [ISO/IEC 9075:1992 §11.3, §13.11] ON COMMIT {DELETE | PRESERVE}
    /// ROWS, if written: whether DELETE.
    pub(super) fn on_commit(&mut self) -> Result<Option<bool>> {
        if !self.accept_kw(kw::ON) {
            return Ok(None);
        }
        self.expect_kw(kw::COMMIT)?;
        let delete_rows = self.accept_kw(kw::DELETE);
        if !delete_rows {
            self.expect_kw(kw::PRESERVE)?;
        }
        self.expect_kw(kw::ROWS)?;
        Ok(Some(delete_rows))
    }

    /// [ISO/IEC 9075:1992 §11.3] `<table element>` ::= `<column definition>`
    /// | `<table constraint definition>`; SQL-86's `<unique constraint
    /// definition>` [X3.135-1986 §6.4] is one of the latter. [ISO/IEC
    /// 9075-2:2023 §11.3] | `<like clause>`.
    fn table_element(&mut self) -> Result<TableElement> {
        let at = self.here();
        if self.accept_kw(kw::LIKE) {
            return self.like_clause(at);
        }
        if self.at_table_constraint() {
            return Ok(TableElement::Constraint(self.table_constraint()?));
        }
        // [ISO/IEC 9075-2:2023 §11.3] a <table period definition>.
        if self.at_period() {
            return Ok(TableElement::Period(self.period_definition()?));
        }
        Ok(TableElement::Column(self.column_definition()?))
    }

    /// [ISO/IEC 9075:1992 §11.4] `<column definition>` ::= `<column name>`
    /// `<data type>` \[`<column constraint definition>`...\], which takes in
    /// SQL-86's \[NOT NULL \[UNIQUE\]\] [X3.135-1986 §6.3].
    pub(super) fn column_definition(&mut self) -> Result<ColumnDefinition> {
        let at = self.here();
        let name = self.identifier()?;
        // [ISO/IEC 9075-2:2023 §11.4] none, before a <generation clause>.
        let data_type = if self.at_generation() { None } else { Some(self.type_name()?) };
        // [ISO/IEC 9075-2:2023 §6.1] of its <predefined type>.
        let of_type = if data_type.is_some() { self.collate_clause()? } else { None };
        let default = if self.accept_kw(kw::DEFAULT) { Some(self.default_option()?) } else { None };
        let generated = if default.is_none() { self.column_generation()? } else { None };
        let mut constraints = Vec::new();
        while self.at_column_constraint() {
            constraints.push(self.column_constraint()?);
        }
        // [ISO/IEC 9075-2:2023 §11.4 SR6c] NOT NULL NOT DEFERRABLE implicit
        // of a system-time period column.
        if let Some(ColumnGeneration::SystemTime { at: stamped, .. }) = &generated
            && !constraints.iter().any(|c| matches!(c.kind, ColumnConstraintKind::NotNull))
        {
            let attributes = ConstraintAttributes { deferrable: false, initially_deferred: false };
            constraints.push(ColumnConstraint { name: None, at: *stamped, kind: ColumnConstraintKind::NotNull, attributes });
        }
        // [ISO/IEC 9075:1992 §11.4] [<collate clause>]
        let collate = self.collate_after(of_type)?;
        Ok(ColumnDefinition { name, at, data_type, default, generated: generated.map(Box::new), constraints, collate })
    }

    /// [ISO/IEC 9075:1992 §11.5] `<default option>` ::= `<literal>` |
    /// `<datetime value function>` | USER | CURRENT_USER | SESSION_USER |
    /// SYSTEM_USER | NULL, after DEFAULT; or [ISO/IEC 9075-2:2023 §11.5]
    /// an `<empty specification>`.
    pub(crate) fn default_option(&mut self) -> Result<DefaultOption> {
        if self.accept_kw(kw::NULL) {
            return Ok(DefaultOption::Null);
        }
        if let Some(kind) = self.empty_specification() {
            return Ok(DefaultOption::Empty(kind));
        }
        if let Some(user) = self.user_function() {
            return Ok(DefaultOption::User(user));
        }
        if let Some(f) = self.datetime_value_function()? {
            return Ok(DefaultOption::Datetime(f));
        }
        Ok(DefaultOption::Literal(self.literal()?))
    }

    /// (`<column name>` \[{,`<column name>`}...\])
    pub(super) fn column_list(&mut self) -> Result<Vec<String>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut columns = vec![self.identifier()?];
        while self.accept_delim(delim::COMMA) {
            columns.push(self.identifier()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(columns)
    }

    /// [ISO/IEC 9075-2:2023 §10.6] `<data type list>` ::= ( \[`<data type>`
    /// \[{, `<data type>`}...\]\] ).
    pub(super) fn data_type_list(&mut self) -> Result<Vec<TypeName>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut types = Vec::new();
        if !self.accept_delim(delim::RIGHT_PAREN) {
            types.push(self.type_name()?);
            while self.accept_delim(delim::COMMA) {
                types.push(self.type_name()?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(types)
    }

    /// `<grantee>` \[{, `<grantee>`}...\]
    pub(super) fn grantees(&mut self) -> Result<Vec<Option<String>>> {
        let mut grantees = vec![self.grantee()?];
        while self.accept_delim(delim::COMMA) {
            grantees.push(self.grantee()?);
        }
        Ok(grantees)
    }

    /// `<grantee>`: an `<authorization identifier>`, or none for PUBLIC.
    fn grantee(&mut self) -> Result<Option<String>> {
        if self.accept_kw(kw::PUBLIC) { Ok(None) } else { self.identifier().map(Some) }
    }
}
