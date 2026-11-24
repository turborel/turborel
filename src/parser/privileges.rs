//! The statements of privileges: [X3.135-1986 §6.6] the `<privilege
//! definition>`, [ISO/IEC 9075:1992 §11.36, §11.37; ISO/IEC 9075-2:2023
//! §12.2, §12.3, §12.7] the `<grant privilege statement>` and the
//! `<revoke statement>` of privileges, with WITH HIERARCHY OPTION and
//! SELECT of a `<privilege method list>` of a table of a structured type.
//! Split from `parser/schema.rs` (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{Action, PrivilegeDefinition, PrivilegeObject, RevokeStatement, RoutineDesignator, TableName, TypeName};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::value::types::XML_NAME;

/// HIERARCHY, of WITH HIERARCHY OPTION and HIERARCHY OPTION FOR [ISO/IEC
/// 9075-2:2023 §12.2, §12.7].
const HIERARCHY: &str = "HIERARCHY";
/// UNDER, an `<action>` [ISO/IEC 9075-2:2023 §12.3].
const UNDER: &str = "UNDER";
/// TRIGGER, an `<action>` [ISO/IEC 9075-2:2023 §12.3].
const TRIGGER: &str = "TRIGGER";
/// \[EXTENSION\] DEBUG, an `<action>` of a routine (RULES.md D1000): a
/// word, not a reserved one, so no name written before it is lost.
const DEBUG: &str = "DEBUG";

/// What ON `<object name>` names: the name, what it names, a routine's
/// `<data type list>` if written, and a method's type.
type ObjectName = (TableName, PrivilegeObject, Option<Vec<TypeName>>, Option<TableName>);

/// The `<privileges>` read: the `<action>`s, none for ALL PRIVILEGES, and
/// the `<privilege method list>` of SELECT, if written.
type Privileges = (Option<Vec<Action>>, Vec<RoutineDesignator>);

impl Parser {
    /// [X3.135-1986 §6.6] `<privilege definition>` ::= GRANT `<privileges>`
    /// ON `<table name>` TO `<grantee>` \[{, `<grantee>`}...\] \[WITH GRANT
    /// OPTION\]; [ISO/IEC 9075:1992 §11.36] `<grant statement>`, its
    /// `<object name>` \[TABLE\] `<table name>`; [ISO/IEC 9075-2:2023
    /// §12.2] \[WITH HIERARCHY OPTION\], of SELECT alone \[SR3a\], and
    /// \[GRANTED BY `<grantor>`\].
    pub(super) fn privilege_definition(&mut self) -> Result<PrivilegeDefinition> {
        self.expect_kw(kw::GRANT)?;
        let (actions, methods) = self.privileges()?;
        let (table, object, parameters, udt) = self.object_name()?;
        self.expect_kw(kw::TO)?;
        let grantees = self.grantees()?;
        let at = self.here();
        let hierarchy_option = self.is_kw(kw::WITH) && matches!(self.peek_at(1), Tok::Identifier(w) if w == HIERARCHY);
        if hierarchy_option {
            self.advance();
            self.advance();
            self.expect_kw(kw::OPTION)?;
            if !actions.as_ref().is_some_and(|list| list.contains(&Action::Select)) {
                let msg = "[ISO/IEC 9075-2:2023 §12.2 SR3a] WITH HIERARCHY OPTION is of an <action> of SELECT without a <privilege column \
                           list> and without a <privilege method list>";
                return Err(SqlError::at(Code::SyntaxRule, at, msg));
            }
        }
        let grant_option = self.accept_kw(kw::WITH);
        if grant_option {
            self.expect_kw(kw::GRANT)?;
            self.expect_kw(kw::OPTION)?;
        }
        let grantor = self.granted_by()?;
        Ok(PrivilegeDefinition { actions, table, object, parameters, grantees, grant_option, hierarchy_option, grantor, methods, udt })
    }

    /// [ISO/IEC 9075:1992 §11.37] `<revoke statement>` ::= REVOKE \[GRANT
    /// OPTION FOR\] `<privileges>` ON `<object name>` FROM `<grantee>` \[{,
    /// `<grantee>`}...\] \[GRANTED BY `<grantor>`\] `<drop behavior>`
    /// [ISO/IEC 9075-2:2023 §12.7], its `<revoke option extension>` GRANT
    /// OPTION FOR or HIERARCHY OPTION FOR.
    pub(super) fn revoke_statement(&mut self) -> Result<RevokeStatement> {
        self.expect_kw(kw::REVOKE)?;
        let grant_option_for = self.accept_kw(kw::GRANT);
        let hierarchy_option_for = !grant_option_for && self.accept_word(HIERARCHY);
        if grant_option_for || hierarchy_option_for {
            self.expect_kw(kw::OPTION)?;
            self.expect_kw(kw::FOR)?;
        }
        let (actions, methods) = self.privileges()?;
        let (table, object, parameters, udt) = self.object_name()?;
        self.expect_kw(kw::FROM)?;
        let grantees = self.grantees()?;
        let grantor = self.granted_by()?;
        let behavior = self.drop_behavior()?;
        Ok(RevokeStatement {
            grant_option_for,
            hierarchy_option_for,
            actions,
            table,
            object,
            parameters,
            grantees,
            grantor,
            methods,
            udt,
            behavior,
        })
    }

    /// `<action>` ::= SELECT \[( `<privilege method list>` )\] | DELETE |
    /// INSERT \[(`<privilege column list>`)\] | UPDATE \[(`<grant column
    /// list>`)\] | REFERENCES \[(`<privilege column list>`)\] | USAGE
    /// [ISO/IEC 9075:1992 §10.3] | EXECUTE | UNDER | TRIGGER [ISO/IEC
    /// 9075-2:2023 §12.3] | DEBUG \[EXTENSION\], a method list's
    /// designators put in `methods`.
    fn action(&mut self, methods: &mut Vec<RoutineDesignator>) -> Result<Action> {
        if self.is_kw(kw::SELECT) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            self.advance();
            self.advance();
            if !self.next_is_routine_designator() {
                return Err(self.syntax("expected a <privilege method list>"));
            }
            loop {
                methods.push(self.routine_designator()?);
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(Action::SelectMethods);
        }
        for (word, action) in
            [(kw::SELECT, Action::Select), (kw::DELETE, Action::Delete), (kw::USAGE, Action::Usage), (kw::EXECUTE, Action::Execute)]
        {
            if self.accept_kw(word) {
                return Ok(action);
            }
        }
        if self.accept_word(UNDER) {
            return Ok(Action::Under);
        }
        if self.accept_word(TRIGGER) {
            return Ok(Action::Trigger);
        }
        if self.accept_word(DEBUG) {
            return Ok(Action::Debug);
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

    /// `<privileges>` ::= ALL PRIVILEGES | `<action>` \[{,`<action>`}...\].
    fn privileges(&mut self) -> Result<Privileges> {
        let mut methods = Vec::new();
        if self.accept_kw(kw::ALL) {
            self.expect_kw(kw::PRIVILEGES)?;
            return Ok((None, methods));
        }
        let mut actions = vec![self.action(&mut methods)?];
        while self.accept_delim(delim::COMMA) {
            actions.push(self.action(&mut methods)?);
        }
        Ok((Some(actions), methods))
    }

    /// ON `<object name>`: [ISO/IEC 9075:1992 §11.36] `<object name>` ::=
    /// \[TABLE\] `<table name>` | DOMAIN `<domain name>` | COLLATION
    /// `<collation name>` | SEQUENCE `<sequence generator name>` | TYPE
    /// `<schema-resolved user-defined type name>` | `<specific routine
    /// designator>` [ISO/IEC 9075-2:2023 §12.3] | \[EXTENSION\] XML SCHEMA
    /// `<registered XML Schema name>` [ISO/IEC 9075-14:2006 draft §4.7.1,
    /// §5.2] | \[EXTENSION\] XQUERY MODULE `<XQuery module name>`, of
    /// those implemented. A
    /// character set or translation is read, its name left empty.
    fn object_name(&mut self) -> Result<ObjectName> {
        self.expect_kw(kw::ON)?;
        let at = self.here();
        // A character set or translation: refused as the statement ends.
        if self.character_set_object()? {
            return Ok((TableName { auth: None, ident: String::new(), at }, PrivilegeObject::Domain, None, None));
        }
        let object = if self.is_word(XML_NAME) && self.next_is_kw(kw::SCHEMA) {
            // [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.7.1] XML SCHEMA
            // <registered XML Schema name>.
            self.advance();
            self.advance();
            PrivilegeObject::XmlSchema
        } else if self.accept_xquery_module() {
            // [EXTENSION] XQUERY MODULE <XQuery module name>.
            PrivilegeObject::XQueryModule
        } else if self.accept_kw(kw::DOMAIN) {
            PrivilegeObject::Domain
        } else if self.accept_kw(kw::COLLATION) {
            PrivilegeObject::Collation
        } else if self.accept_word(super::sequences::SEQUENCE) {
            PrivilegeObject::Sequence
        } else if self.accept_word(super::user_types::TYPE) {
            PrivilegeObject::Type
        } else if self.graph_at(0) {
            // [EXTENSION] PROPERTY GRAPH <property graph name>.
            self.advance();
            self.advance();
            PrivilegeObject::Graph
        } else if self.is_kw(kw::MODULE) && !matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD)) {
            // [ISO/IEC 9075-4:2006 draft §10.2] MODULE <module name>.
            self.advance();
            PrivilegeObject::Module
        } else if self.accept_model_object() {
            PrivilegeObject::Model
        } else if self.next_is_routine_designator() {
            let designator = self.routine_designator()?;
            let object = if designator.specific {
                PrivilegeObject::SpecificRoutine(designator.routine_type)
            } else {
                PrivilegeObject::Routine(designator.routine_type)
            };
            return Ok((designator.name, object, designator.parameters, designator.udt));
        } else {
            self.accept_kw(kw::TABLE);
            PrivilegeObject::Table
        };
        Ok((self.table_name()?, object, None, None))
    }
}
