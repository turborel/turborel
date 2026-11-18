//! \[EXTENSION\] The grammar of registered XML Schemas, which Part 14
//! leaves to the implementation [ISO/IEC 9075-14:2006 draft §4.2.6,
//! §4.7.1] (RULES.md D86): the `<XML Schema definition>` and the `<drop
//! XML Schema statement>` (`parser/ddl.rs`).
//!
//! XML is no key word of SQL-92, so a table may be named XML; none is
//! followed by SCHEMA where these read XML SCHEMA, so the two words are
//! read together.

use super::super::Parser;
use super::LOCATION;
use crate::ast::{SchemaStatement, XmlSchemaDefinition};
use crate::error::Result;
use crate::lexer::{Tok, kw};
use crate::value::types::XML_NAME;

impl Parser {
    /// Whether XML SCHEMA begins `k` tokens ahead.
    pub(in crate::parser) fn at_xml_schema(&self, k: usize) -> bool {
        let Tok::Identifier(word) = self.peek_at(k) else { return false };
        word == XML_NAME && matches!(self.peek_at(k.saturating_add(1)), Tok::KeyWord(kw::SCHEMA))
    }

    /// XML SCHEMA, read if next: whether it was.
    pub(in crate::parser) fn accept_xml_schema(&mut self) -> bool {
        let at = self.at_xml_schema(0);
        if at {
            self.advance();
            self.advance();
        }
        at
    }

    /// An `<XML Schema definition>`, if CREATE XML SCHEMA is next, else a
    /// definition of a type's if one is (`defined_for_type`).
    ///
    /// # Errors
    ///
    /// The text is not the definition its first words begin.
    pub(in crate::parser) fn defined_elsewhere(&mut self) -> Result<Option<SchemaStatement>> {
        if self.at_xml_schema(1) {
            return Ok(Some(SchemaStatement::XmlSchema(Box::new(self.xml_schema_definition()?))));
        }
        self.defined_for_type()
    }

    /// `<XML Schema definition>` ::= CREATE XML SCHEMA `<registered XML
    /// Schema name>` \[ LOCATION `<XML valid schema location URI>` \] AS
    /// `<XML Schema document>`; `<XML Schema document>` ::= `<value
    /// expression>`; `<registered XML Schema name>` ::= `<schema
    /// qualified name>` [§5.2]; `<XML valid schema location URI>` ::=
    /// `<character string literal>` [§11.6].
    fn xml_schema_definition(&mut self) -> Result<XmlSchemaDefinition> {
        self.expect_kw(kw::CREATE)?;
        // XML SCHEMA, which `defined_elsewhere` found.
        self.advance();
        self.advance();
        let name = self.table_name()?;
        let location =
            if self.accept_word(LOCATION) { Some(self.character_string_literal("<XML valid schema location URI>")?) } else { None };
        self.expect_kw(kw::AS)?;
        let document = self.value_expression()?;
        Ok(XmlSchemaDefinition { name, location, document })
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::{DropBehavior, DropObject, SchemaStatement, SqlStatement, TableName, XmlSchemaDefinition};
    use crate::error::Result;
    use crate::parser::Parser;

    /// `text`, parsed as one statement.
    fn parsed(text: &str) -> Result<SqlStatement> {
        let mut parser = Parser::new(text)?;
        let statement = parser.sql_statement()?;
        parser.end()?;
        Ok(statement)
    }

    /// The `<XML Schema definition>` `text` is, which must parse.
    fn definition(text: &str) -> XmlSchemaDefinition {
        match parsed(text) {
            Ok(SqlStatement::Schema { statement: SchemaStatement::XmlSchema(x), .. }) => *x,
            other => panic!("{text}: {other:?}"),
        }
    }

    /// A name's schema, if written, and identifier.
    fn name(n: &TableName) -> (Option<&str>, &str) {
        (n.auth.as_deref(), n.ident.as_str())
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.2.6, §5.2,
    /// §11.6] RULES.md D86: CREATE XML SCHEMA reads a `<schema qualified
    /// name>`, LOCATION and its `<character string literal>` if written,
    /// and AS any `<value expression>`; DROP XML SCHEMA its name and a
    /// `<drop behavior>`, which it must have. A table named XML is still a
    /// table, as XML is no key word.
    #[test]
    fn the_statements_of_registered_schemas_parse() {
        let x = definition("CREATE XML SCHEMA A.S LOCATION 'http://e.org/s.xsd' AS '<xs:schema/>'");
        assert_eq!((name(&x.name), x.location.as_deref()), ((Some("A"), "S"), Some("http://e.org/s.xsd")));
        let y = definition("create xml schema S as XMLPARSE(DOCUMENT D PRESERVE WHITESPACE)");
        assert_eq!((name(&y.name), y.location), ((None, "S"), None));
        let Ok(SqlStatement::Drop { object: DropObject::XmlSchema(n), behavior }) = parsed("DROP XML SCHEMA A.S CASCADE") else {
            panic!("no drop")
        };
        assert_eq!((name(&n), behavior), ((Some("A"), "S"), DropBehavior::Cascade));
        assert!(parsed("DROP XML SCHEMA S").is_err(), "a <drop behavior> is required");
        assert!(parsed("CREATE XML SCHEMA S LOCATION 7 AS 'x'").is_err(), "LOCATION takes a <character string literal>");
        assert!(matches!(parsed("CREATE TABLE XML (A INT)"), Ok(SqlStatement::Schema { statement: SchemaStatement::Table(_), .. })));
        assert!(matches!(parsed("DROP TABLE XML RESTRICT"), Ok(SqlStatement::Drop { object: DropObject::Table(_), .. })));
    }
}
