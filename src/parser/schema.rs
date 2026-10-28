//! The schema grammar: `<schema>` [X3.135-1986 §6.1], its `<table
//! definition>`s [X3.135-1986 §6.2-§6.4] and `<privilege definition>`s
//! [X3.135-1986 §6.6]. Split from parser.rs (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{
    Action, CheckLevels, ColumnDefinition, DefaultOption, PrivilegeDefinition, QueryExpression, RevokeStatement, Schema, SchemaStatement,
    SqlStatement, TableDefinition, TableElement, TableName, TableStorage, Temporary, TemporaryKind, ViewDefinition,
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
        // <schema name clause> ::= <schema name> | AUTHORIZATION <schema
        // authorization identifier> | <schema name> AUTHORIZATION <schema
        // authorization identifier>
        let named = if self.is_kw(kw::AUTHORIZATION) { None } else { Some(self.schema_name()?) };
        let auth = if self.accept_kw(kw::AUTHORIZATION) { Some(self.identifier()?) } else { None };
        // [§11.1 SR1] no <schema name>: the <schema authorization identifier>.
        let (catalog, name) = match (named, &auth) {
            (Some((catalog, name, _)), _) => (catalog, name),
            (None, Some(auth)) => (None, auth.clone()),
            (None, None) => return Err(self.syntax("expected a <schema name> or AUTHORIZATION")),
        };
        // [§11.1] <schema character set specification> ::= DEFAULT
        // CHARACTER SET <character set specification>: SQL_TEXT's taken.
        if self.accept_kw(kw::DEFAULT) {
            if !self.is_kw(kw::CHARACTER) {
                return Err(self.syntax("expected CHARACTER SET"));
            }
            self.character_set_clause()?;
        }

        // <schema element> ::= <table definition> | <view definition> | <privilege definition>
        let (mut domains, mut tables, mut views, mut privileges) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut assertions = Vec::new();
        loop {
            if self.is_kw(kw::GRANT) {
                privileges.push(self.privilege_definition()?);
                continue;
            }
            if !self.is_kw(kw::CREATE) {
                break;
            }
            // [§11.1] a character set, collation or translation
            // definition: read, then refused.
            if matches!(self.peek_at(1), Tok::KeyWord(kw::CHARACTER | kw::COLLATION | kw::TRANSLATION)) {
                self.advance();
                self.character_set_definition()?;
                continue;
            }
            if matches!(self.peek_at(1), Tok::KeyWord(kw::VIEW)) {
                views.push(self.view_definition()?);
            } else if matches!(self.peek_at(1), Tok::KeyWord(kw::DOMAIN)) {
                domains.push(self.domain_definition()?);
            } else if matches!(self.peek_at(1), Tok::KeyWord(kw::ASSERTION)) {
                assertions.push(self.assertion_definition()?);
            } else {
                tables.push(self.table_definition()?);
            }
        }
        self.end()?;
        Ok(Schema { catalog, name, auth, domains, assertions, tables, views, privileges })
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

    /// \[EXTENSION\] [ISO/IEC 9075:1992 §20.1] A `<table definition>`, a
    /// `<view definition>`, a `<grant statement>` or a `<revoke statement>`,
    /// on its own: with its text.
    pub(super) fn schema_statement(&mut self) -> Result<SqlStatement> {
        let start = self.here();
        let statement = if self.is_kw(kw::GRANT) {
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
        } else {
            SchemaStatement::Table(self.table_definition()?)
        };
        Ok(SqlStatement::Schema { statement, text: self.text_from(start) })
    }

    /// [ISO/IEC 9075:1992 §11.37] `<revoke statement>` ::= REVOKE \[GRANT
    /// OPTION FOR\] `<privileges>` ON `<object name>` FROM `<grantee>` \[{,
    /// `<grantee>`}...\] `<drop behavior>`
    fn revoke_statement(&mut self) -> Result<RevokeStatement> {
        self.expect_kw(kw::REVOKE)?;
        let grant_option_for = self.accept_kw(kw::GRANT);
        if grant_option_for {
            self.expect_kw(kw::OPTION)?;
            self.expect_kw(kw::FOR)?;
        }
        let actions = self.privileges()?;
        let (table, domain) = self.object_name()?;
        self.expect_kw(kw::FROM)?;
        let grantees = self.grantees()?;
        Ok(RevokeStatement { grant_option_for, actions, table, domain, grantees, behavior: self.drop_behavior()? })
    }

    /// [X3.135-1986 §6.5; ISO/IEC 9075:1992 §11.19] `<view definition>` ::=
    /// CREATE VIEW `<table name>` \[(`<view column list>`)\] AS `<query
    /// expression>` \[WITH \[`<levels clause>`\] CHECK OPTION\]
    fn view_definition(&mut self) -> Result<ViewDefinition> {
        self.expect_kw(kw::CREATE)?;
        self.expect_kw(kw::VIEW)?;
        let name = self.table_name()?;
        // <view column list> ::= <column name> [{,<column name>}...]
        let columns = if self.is_delim(delim::LEFT_PAREN) { Some(self.column_list()?) } else { None };
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
        Ok(ViewDefinition { name, columns, query, text, check_option })
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
    ///                            (`<table element>` \[{,`<table element>`}...\])
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
        let elements = self.table_element_list()?;
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
        Ok(TableDefinition { name, temporary, elements, storage })
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
        let name = TableName { auth: Some(kw::MODULE.to_owned()), ident: self.identifier()?, at };
        let elements = self.table_element_list()?;
        let delete_rows = self.on_commit()?.unwrap_or(true);
        let temporary = Some(Temporary { kind: TemporaryKind::Declared, delete_rows });
        Ok(TableDefinition { name, temporary, elements, storage: TableStorage::default() })
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
    /// definition>` [X3.135-1986 §6.4] is one of the latter.
    fn table_element(&mut self) -> Result<TableElement> {
        if self.at_table_constraint() {
            return Ok(TableElement::Constraint(self.table_constraint()?));
        }
        Ok(TableElement::Column(self.column_definition()?))
    }

    /// [ISO/IEC 9075:1992 §11.4] `<column definition>` ::= `<column name>`
    /// `<data type>` \[`<column constraint definition>`...\], which takes in
    /// SQL-86's \[NOT NULL \[UNIQUE\]\] [X3.135-1986 §6.3].
    pub(super) fn column_definition(&mut self) -> Result<ColumnDefinition> {
        let at = self.here();
        let name = self.identifier()?;
        let data_type = self.type_name()?;
        let default = if self.accept_kw(kw::DEFAULT) { Some(self.default_option()?) } else { None };
        let mut constraints = Vec::new();
        while self.at_column_constraint() {
            constraints.push(self.column_constraint()?);
        }
        // [ISO/IEC 9075:1992 §11.4] [<collate clause>]: refused.
        self.collate_clause()?;
        Ok(ColumnDefinition { name, at, data_type, default, constraints })
    }

    /// [ISO/IEC 9075:1992 §11.5] `<default option>` ::= `<literal>` |
    /// `<datetime value function>` | USER | CURRENT_USER | SESSION_USER |
    /// SYSTEM_USER | NULL, after DEFAULT.
    pub(super) fn default_option(&mut self) -> Result<DefaultOption> {
        if self.accept_kw(kw::NULL) {
            return Ok(DefaultOption::Null);
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
    fn column_list(&mut self) -> Result<Vec<String>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut columns = vec![self.identifier()?];
        while self.accept_delim(delim::COMMA) {
            columns.push(self.identifier()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(columns)
    }

    /// [X3.135-1986 §6.6] `<privilege definition>` ::= GRANT `<privileges>`
    /// ON `<table name>` TO `<grantee>` \[{, `<grantee>`}...\] \[WITH GRANT
    /// OPTION\]; [ISO/IEC 9075:1992 §11.36] `<grant statement>`, its
    /// `<object name>` \[TABLE\] `<table name>`.
    fn privilege_definition(&mut self) -> Result<PrivilegeDefinition> {
        self.expect_kw(kw::GRANT)?;
        let actions = self.privileges()?;
        let (table, domain) = self.object_name()?;
        self.expect_kw(kw::TO)?;
        let grantees = self.grantees()?;
        let grant_option = self.accept_kw(kw::WITH);
        if grant_option {
            self.expect_kw(kw::GRANT)?;
            self.expect_kw(kw::OPTION)?;
        }
        Ok(PrivilegeDefinition { actions, table, domain, grantees, grant_option })
    }

    /// `<action>` ::= SELECT | DELETE | INSERT \[(`<privilege column
    /// list>`)\] | UPDATE \[(`<grant column list>`)\] | REFERENCES
    /// \[(`<privilege column list>`)\] [ISO/IEC 9075:1992 §10.3]
    fn action(&mut self) -> Result<Action> {
        for (word, action) in [(kw::SELECT, Action::Select), (kw::DELETE, Action::Delete), (kw::USAGE, Action::Usage)] {
            if self.accept_kw(word) {
                return Ok(action);
            }
        }
        let made = if self.accept_kw(kw::INSERT) {
            Action::Insert
        } else if self.accept_kw(kw::REFERENCES) {
            Action::References
        } else {
            self.expect_kw(kw::UPDATE)?;
            Action::Update
        };
        // <privilege column list> ::= <column name> [{,<column name>}...]
        let columns = if self.is_delim(delim::LEFT_PAREN) { Some(self.column_list()?) } else { None };
        Ok(made(columns))
    }

    /// `<privileges>` ::= ALL PRIVILEGES | `<action>` \[{,`<action>`}...\]:
    /// none for ALL PRIVILEGES.
    fn privileges(&mut self) -> Result<Option<Vec<Action>>> {
        if self.accept_kw(kw::ALL) {
            self.expect_kw(kw::PRIVILEGES)?;
            return Ok(None);
        }
        let mut actions = vec![self.action()?];
        while self.accept_delim(delim::COMMA) {
            actions.push(self.action()?);
        }
        Ok(Some(actions))
    }

    /// ON `<object name>`: [ISO/IEC 9075:1992 §11.36] `<object name>` ::=
    /// \[TABLE\] `<table name>`, of those implemented.
    fn object_name(&mut self) -> Result<(TableName, bool)> {
        self.expect_kw(kw::ON)?;
        let at = self.here();
        // A collation, character set or translation: refused as the
        // statement ends.
        if self.character_set_object()? {
            return Ok((TableName { auth: None, ident: String::new(), at }, true));
        }
        if self.accept_kw(kw::DOMAIN) {
            return Ok((self.table_name()?, true));
        }
        self.accept_kw(kw::TABLE);
        Ok((self.table_name()?, false))
    }

    /// `<grantee>` \[{, `<grantee>`}...\]
    fn grantees(&mut self) -> Result<Vec<Option<String>>> {
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
