//! \[EXTENSION\] The grammar of registered XQuery modules (RULES.md D106),
//! which \[XQuery\] leaves to the implementation [XQuery §5.2.5]: the
//! `<XQuery module definition>` and the `<drop XQuery module statement>`
//! (`parser/ddl.rs`).
//!
//! XQUERY is no key word, so a table may be named XQUERY; MODULE follows
//! no table name where these read XQUERY MODULE, so the two words are read
//! together.

use super::super::Parser;
use super::valid::LOCATION;
use crate::ast::XQueryModuleDefinition;
use crate::error::Result;
use crate::lexer::{Tok, kw};

/// The word XQUERY, of XQUERY MODULE.
pub(in crate::parser) const XQUERY: &str = "XQUERY";

impl Parser {
    /// Whether XQUERY MODULE begins `k` tokens ahead.
    pub(in crate::parser) fn at_xquery_module(&self, k: usize) -> bool {
        let Tok::Identifier(word) = self.peek_at(k) else { return false };
        word == XQUERY && matches!(self.peek_at(k.saturating_add(1)), Tok::KeyWord(kw::MODULE))
    }

    /// XQUERY MODULE, read if next: whether it was.
    pub(in crate::parser) fn accept_xquery_module(&mut self) -> bool {
        let at = self.at_xquery_module(0);
        if at {
            self.advance();
            self.advance();
        }
        at
    }

    /// `<XQuery module definition>` ::= CREATE XQUERY MODULE `<XQuery
    /// module name>` \[ LOCATION `<XQuery module location>` \] AS
    /// `<XQuery module text>`; `<XQuery module name>` ::= `<schema
    /// qualified name>`; `<XQuery module location>` ::= `<character string
    /// literal>`; `<XQuery module text>` ::= `<character string literal>`.
    pub(in crate::parser) fn xquery_module_definition(&mut self) -> Result<XQueryModuleDefinition> {
        self.expect_kw(kw::CREATE)?;
        // XQUERY MODULE, which `defined_elsewhere` found.
        self.advance();
        self.advance();
        let name = self.table_name()?;
        let location = if self.accept_word(LOCATION) { Some(self.character_string_literal("<XQuery module location>")?) } else { None };
        self.expect_kw(kw::AS)?;
        let text = self.character_string_literal("<XQuery module text>")?;
        Ok(XQueryModuleDefinition { name, location, text })
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::{DropBehavior, DropObject, SchemaStatement, SqlStatement, TableName, XQueryModuleDefinition};
    use crate::error::Result;
    use crate::parser::Parser;

    /// `text`, parsed as one statement.
    fn parsed(text: &str) -> Result<SqlStatement> {
        let mut parser = Parser::new(text)?;
        let statement = parser.sql_statement()?;
        parser.end()?;
        Ok(statement)
    }

    /// The `<XQuery module definition>` `text` is, which must parse.
    fn definition(text: &str) -> XQueryModuleDefinition {
        match parsed(text) {
            Ok(SqlStatement::Schema { statement: SchemaStatement::XQueryModule(m), .. }) => *m,
            other => panic!("{text}: {other:?}"),
        }
    }

    /// A name's schema, if written, and identifier.
    fn name(n: &TableName) -> (Option<&str>, &str) {
        (n.auth.as_deref(), n.ident.as_str())
    }

    /// Verifies: [EXTENSION] RULES.md D106 [XQuery §5.2.5]: CREATE XQUERY
    /// MODULE reads a `<schema qualified name>`, LOCATION and its
    /// `<character string literal>` if written, and AS a `<character
    /// string literal>`, of quoted parts joined; DROP XQUERY MODULE its
    /// name and a `<drop behavior>`, which it must have. A table named
    /// XQUERY is still a table, as XQUERY is no key word.
    #[test]
    fn the_statements_of_registered_modules_parse() {
        let m =
            definition("CREATE XQUERY MODULE A.M LOCATION 'm.xq' AS 'module namespace m = \"urn:m\";'\n' declare function m:f() { 1 };'");
        assert_eq!((name(&m.name), m.location.as_deref()), ((Some("A"), "M"), Some("m.xq")));
        assert_eq!(m.text, "module namespace m = \"urn:m\"; declare function m:f() { 1 };");
        let n = definition("create xquery module N as 'module namespace n = \"urn:n\";'");
        assert_eq!((name(&n.name), n.location), ((None, "N"), None));
        let Ok(SqlStatement::Drop { object: DropObject::XQueryModule(d), behavior }) = parsed("DROP XQUERY MODULE A.M RESTRICT") else {
            panic!("no drop")
        };
        assert_eq!((name(&d), behavior), ((Some("A"), "M"), DropBehavior::Restrict));
        assert!(parsed("DROP XQUERY MODULE M").is_err(), "a <drop behavior> is required");
        assert!(parsed("CREATE XQUERY MODULE M AS 1").is_err(), "AS takes a <character string literal>");
        assert!(matches!(parsed("CREATE TABLE XQUERY (A INT)"), Ok(SqlStatement::Schema { statement: SchemaStatement::Table(_), .. })));
        assert!(matches!(parsed("DROP TABLE XQUERY RESTRICT"), Ok(SqlStatement::Drop { object: DropObject::Table(_), .. })));
    }
}
