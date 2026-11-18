//! The grammar of the SQL/XML operators that evaluate XQuery [ISO/IEC
//! 9075-14:2006 draft §6.18, §7.1, §8.4]: XMLQUERY, XMLEXISTS, read as a
//! boolean value as a `<boolean predicand>` is, and XMLTABLE, a `<table
//! primary>` read as the one collection of a collection derived table
//! whose rows it gives.

use super::super::Parser;
use super::publish::XMLNAMESPACES;
use crate::ast::{
    OnEmpty, TableReference, UnnestTable, ValueExpression, XmlArgument, XmlArguments, XmlOperator, XmlQuery, XmlRegularColumn, XmlTable,
    XmlTableColumn,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

// [§5.1] The words of SQL/XML this grammar reads.
/// `<XML query>`'s.
pub const XMLQUERY: &str = "XMLQUERY";
/// `<XML exists predicate>`'s.
pub const XMLEXISTS: &str = "XMLEXISTS";
/// `<XML table>`'s.
pub const XMLTABLE: &str = "XMLTABLE";
/// PASSING, of an argument list.
const PASSING: &str = "PASSING";
/// EMPTY, of EMPTY ON EMPTY.
const EMPTY: &str = "EMPTY";
/// COLUMNS.
const COLUMNS: &str = "COLUMNS";
/// ORDINALITY, of FOR ORDINALITY.
const ORDINALITY: &str = "ORDINALITY";
/// PATH.
const PATH: &str = "PATH";

impl Parser {
    /// [§6.18] `<XML query>` ::= XMLQUERY ( `<XQuery expression>` \[`<XML
    /// query argument list>`\] \[`<XML returning clause>` \[`<XML query
    /// returning mechanism>`\]\] `<XML query empty handling option>` ).
    pub(super) fn xml_query(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_word(XMLQUERY)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let text = self.character_string_literal("<XQuery expression>")?;
        let arguments = self.xml_arguments()?;
        let returning = self.returning_clause()?;
        let mechanism = if returning.is_some() { self.passing_mechanism()? } else { None };
        let empty = if self.accept_kw(kw::NULL) {
            OnEmpty::Null
        } else if self.accept_word(EMPTY) {
            OnEmpty::Empty
        } else {
            return Err(self.syntax("expected NULL ON EMPTY or EMPTY ON EMPTY [ISO/IEC 9075-14:2006 draft §6.18]"));
        };
        self.expect_kw(kw::ON)?;
        self.expect_word(EMPTY)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let query = XmlQuery { text, arguments, returning, mechanism, empty: Some(empty) };
        Ok(ValueExpression::Xml(Box::new(XmlOperator::Query(query)), at))
    }

    /// [§8.4] `<XML exists predicate>` ::= XMLEXISTS ( `<XQuery expression>`
    /// \[`<XML query argument list>`\] ).
    pub(super) fn xml_exists(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_word(XMLEXISTS)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let text = self.character_string_literal("<XQuery expression>")?;
        let arguments = self.xml_arguments()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let query = XmlQuery { text, arguments, returning: None, mechanism: None, empty: None };
        Ok(ValueExpression::Xml(Box::new(XmlOperator::Exists(query)), at))
    }

    /// [§6.18, §7.1] `<XML query argument list>` or `<XML table argument
    /// list>` ::= PASSING `<XML passing mechanism>` `<XML query argument>`
    /// \[ { , `<XML query argument>` }... \], if next; each argument a
    /// `<value expression>` \[AS `<identifier>`\] \[`<XML passing
    /// mechanism>`\].
    fn xml_arguments(&mut self) -> Result<Option<XmlArguments>> {
        if !self.accept_word(PASSING) {
            return Ok(None);
        }
        let Some(default) = self.passing_mechanism()? else {
            return Err(self.syntax("expected BY REF or BY VALUE after PASSING [ISO/IEC 9075-14:2006 draft §6.18]"));
        };
        let mut arguments = Vec::new();
        loop {
            let at = self.here();
            let value = self.value_expression()?;
            let name = if self.accept_kw(kw::AS) { Some(self.identifier()?) } else { None };
            let passing = self.passing_mechanism()?;
            arguments.push(XmlArgument { value, name, passing, at });
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        Ok(Some(XmlArguments { default, arguments }))
    }

    /// Whether an `<XML table>` begins here: XMLTABLE, no key word of
    /// SQL-92, before (.
    pub(in crate::parser) fn at_xml_table(&self) -> bool {
        self.at_xml(XMLTABLE)
    }

    /// [§7.1] `<XML table>` \[AS\] `<correlation name>` \[( `<derived
    /// column list>` )\], read as a collection derived table of the one
    /// collection the XMLTABLE is, its correlation name not optional.
    pub(in crate::parser) fn xml_table(&mut self) -> Result<TableReference> {
        let at = self.here();
        self.expect_word(XMLTABLE)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let namespaces = if self.at_xml(XMLNAMESPACES) {
            let items = self.xml_namespace_declaration()?;
            self.expect_delim(delim::COMMA)?;
            Some(items)
        } else {
            None
        };
        let pattern = self.character_string_literal("<XML table row pattern>")?;
        let arguments = self.xml_arguments()?;
        self.expect_word(COLUMNS)?;
        let mut columns = vec![self.xml_table_column()?];
        while self.accept_delim(delim::COMMA) {
            columns.push(self.xml_table_column()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        let table = ValueExpression::Xml(Box::new(XmlOperator::Table(XmlTable { namespaces, pattern, arguments, columns })), at);
        let correlation =
            self.correlation()?.ok_or_else(|| self.syntax("[ISO/IEC 9075-14:2006 draft §7.1] an <XML table> has a correlation name"))?;
        let derived = self.derived_columns()?;
        Ok(TableReference::Unnest(UnnestTable { collections: vec![table], ordinality: false, correlation, columns: derived, at }))
    }

    /// [§7.1] `<XML table column definition>`: `<column name>` FOR
    /// ORDINALITY, or `<column name>` `<data type>` \[`<XML passing
    /// mechanism>`\] \[DEFAULT `<default option>`\] \[PATH `<XML table
    /// column pattern>`\].
    fn xml_table_column(&mut self) -> Result<XmlTableColumn> {
        let at = self.here();
        let name = self.identifier()?;
        let ordinality = self.is_kw(kw::FOR) && if let Tok::Identifier(w) = self.peek_at(1) { w == ORDINALITY } else { false };
        if ordinality {
            self.advance();
            self.advance();
            return Ok(XmlTableColumn::Ordinality(name));
        }
        let data_type = self.type_name()?;
        let passing = self.passing_mechanism()?;
        let default = if self.accept_kw(kw::DEFAULT) { Some(self.default_option()?) } else { None };
        let path = if self.accept_word(PATH) { Some(self.character_string_literal("<XML table column pattern>")?) } else { None };
        Ok(XmlTableColumn::Regular(XmlRegularColumn { name, data_type, passing, default, path, at }))
    }
}
