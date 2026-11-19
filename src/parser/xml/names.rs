//! \[EXTENSION\] The syntax of Feature X400's mappings of names [ISO/IEC
//! 9075-14:2006 draft §9.1, §9.3], which the draft leaves to applications
//! [§4.10.3 NOTE 10, §4.10.4 NOTE 11] (RULES.md D106):
//!
//! ```text
//! <XML name to identifier> ::=
//!     XMLNAME_TO_IDENTIFIER <left paren> <character value expression> <right paren>
//!
//! <identifier to XML name> ::=
//!     IDENTIFIER_TO_XMLNAME <left paren> <character value expression>
//!         [ <comma> { PARTIALLY | FULLY } ESCAPED ] <right paren>
//! ```
//!
//! Each is a call of the function registered under its name
//! (`functions/xml/names.rs`), PARTIALLY ESCAPED if no variant is written.
//! The words are no key words, each read where the grammar takes it.

use super::super::Parser;
use crate::ast::ValueExpression;
use crate::error::Result;
use crate::functions::{IDENTIFIER_TO_XMLNAME, IDENTIFIER_TO_XMLNAME_FULLY, XMLNAME_TO_IDENTIFIER};
use crate::lexer::delim;

/// PARTIALLY, of PARTIALLY ESCAPED.
const PARTIALLY: &str = "PARTIALLY";
/// FULLY, of FULLY ESCAPED.
const FULLY: &str = "FULLY";
/// ESCAPED.
const ESCAPED: &str = "ESCAPED";

impl Parser {
    /// An `<XML name to identifier>` or an `<identifier to XML name>`, if
    /// one is next: its word, then "(".
    pub(super) fn xml_name_mapping(&mut self) -> Result<Option<ValueExpression>> {
        let to_identifier = self.at_xml(XMLNAME_TO_IDENTIFIER);
        if !to_identifier && !self.at_xml(IDENTIFIER_TO_XMLNAME) {
            return Ok(None);
        }
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let args = vec![self.value_expression()?];
        let name = if to_identifier { XMLNAME_TO_IDENTIFIER } else { self.escape_variant()? };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Some(ValueExpression::Function { name, args, at }))
    }

    /// The function an `<identifier to XML name>` calls: of `, FULLY
    /// ESCAPED` the fully escaped mapping, else the partially escaped one.
    fn escape_variant(&mut self) -> Result<&'static str> {
        if !self.accept_delim(delim::COMMA) {
            return Ok(IDENTIFIER_TO_XMLNAME);
        }
        let name = if self.accept_word(FULLY) {
            IDENTIFIER_TO_XMLNAME_FULLY
        } else if self.accept_word(PARTIALLY) {
            IDENTIFIER_TO_XMLNAME
        } else {
            return Err(self.syntax("expected PARTIALLY or FULLY [EXTENSION]"));
        };
        if !self.accept_word(ESCAPED) {
            return Err(self.syntax("expected ESCAPED [EXTENSION]"));
        }
        Ok(name)
    }
}
