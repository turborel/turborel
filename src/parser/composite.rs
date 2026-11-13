//! Arrays and rows: [ISO/IEC 9075-2:2023 §6.1] a `<data type>`, a
//! `<predefined type>`, a `<row type>`, or a `<collection type>` of
//! either, an `<array type>`; and the expressions of them [§6.16, §6.25,
//! §6.31, §6.47, §6.48, §6.49, §7.1, §10.9].

use std::collections::HashSet;

use super::Parser;
use crate::ast::{
    ColumnSpecification, Composite, SelectSublist, SetArg, SetFunctionSpecification, Subquery, TableName, TableReference, UnnestTable,
    ValueExpression,
};
use crate::error::{Code, Result, SqlError};
use crate::functions::{ARRAY_AGG, ARRAY_MAX_CARDINALITY, CARDINALITY, TRIM_ARRAY};
use crate::lexer::{Tok, delim, kw};
use crate::value::DataType;
use crate::value::types::composite::nesting;
use crate::value::types::{ARRAY_NAME, Field, MAX_ARRAY_CARDINALITY, MAX_TYPE_NESTING, ROW_NAME};

/// [ISO/IEC 9075-2:2023 §7.6] The words of a `<collection derived table>`,
/// no key words of SQL-92.
const UNNEST: &str = "UNNEST";
const ORDINALITY: &str = "ORDINALITY";

/// [ISO/IEC 9075-2:2023 §6.5] The tokens of an `<empty specification>`,
/// ARRAY `[` `]`.
const EMPTY_SPECIFICATION_TOKENS: usize = 3;

impl Parser {
    /// [ISO/IEC 9075-2:2023 §6.1] `<data type>` ::= `<predefined type>` |
    /// `<row type>` | `<collection type>`, `<array type>` ::= `<data type>`
    /// ARRAY \[`[` `<maximum cardinality>` `]`\], so ARRAY may follow
    /// another: nested no deeper than `MAX_TYPE_NESTING`.
    pub(super) fn data_type(&mut self) -> Result<DataType> {
        let at = self.here();
        let t = if self.at_row_type() { self.row_type()? } else { self.predefined_type()? };
        let t = self.array_suffix()?.into_iter().fold(t, DataType::array);
        within_nesting(nesting(&t), at)?;
        Ok(t)
    }

    /// [ISO/IEC 9075-2:2023 §6.1] The maximum cardinality of each ARRAY
    /// \[`[` `<maximum cardinality>` `]`\] that follows a `<data type>`,
    /// in order: of one written without it, `MAX_ARRAY_CARDINALITY` [SR47].
    pub(super) fn array_suffix(&mut self) -> Result<Vec<u32>> {
        let mut maxima = Vec::new();
        while self.accept_word(ARRAY_NAME) {
            maxima.push(if self.accept_delim(delim::LEFT_BRACKET) { self.maximum_cardinality()? } else { MAX_ARRAY_CARDINALITY });
        }
        Ok(maxima)
    }

    /// Whether a `<row type>` begins here: ROW, no key word of SQL-92,
    /// before (.
    pub(super) fn at_row_type(&self) -> bool {
        self.is_word(ROW_NAME) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
    }

    /// [ISO/IEC 9075-2:2023 §6.1] `<maximum cardinality>` ::= `<unsigned
    /// integer>`, then `]`: more than 0, and no more than this
    /// implementation's (IL008).
    fn maximum_cardinality(&mut self) -> Result<u32> {
        let at = self.here();
        let max = self.positive("maximum cardinality")?;
        self.expect_delim(delim::RIGHT_BRACKET)?;
        if max > MAX_ARRAY_CARDINALITY {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!(
                    "[ISO/IEC 9075-2:2023 §6.1 SR47] an array of {max} elements exceeds this implementation's maximum cardinality of {MAX_ARRAY_CARDINALITY}"
                ),
            ));
        }
        Ok(max)
    }

    /// [ISO/IEC 9075-2:2023 §6.1] `<row type>` ::= ROW ( `<field
    /// definition>` \[, ...\] ), `<field definition>` ::= `<field name>`
    /// `<data type>` [§6.2], no two names equivalent \[SR2\].
    fn row_type(&mut self) -> Result<DataType> {
        let fields = self.row_fields(Parser::data_type)?;
        Ok(DataType::row(fields.into_iter().map(|(name, data_type)| Field { name, data_type, unnamed: false }).collect()))
    }

    /// [ISO/IEC 9075-2:2023 §6.1, §6.2] The fields of a `<row type>`, each
    /// its name and its type as `part` reads it, no two names equivalent
    /// \[SR2\].
    pub(super) fn row_fields<T>(&mut self, part: fn(&mut Parser) -> Result<T>) -> Result<Vec<(String, T)>> {
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut fields = Vec::new();
        let mut names = HashSet::new();
        loop {
            let at = self.here();
            let name = self.identifier()?;
            if !names.insert(name.clone()) {
                return Err(SqlError::at(
                    Code::SyntaxRule,
                    at,
                    format!("[ISO/IEC 9075-2:2023 §6.2 SR2] the row type has two fields {name}"),
                ));
            }
            fields.push((name, self.nested(part)?));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(fields)
    }
}

/// That a type written at `at` nests arrays and rows `depth` deep, no
/// deeper than `MAX_TYPE_NESTING`.
///
/// # Errors
///
/// SQLCODE -900 if it nests deeper.
pub(super) fn within_nesting(depth: usize, at: usize) -> Result<()> {
    if depth > MAX_TYPE_NESTING {
        return Err(SqlError::at(
            Code::Unsupported,
            at,
            format!("[DESIGN] the type nests arrays and rows deeper than this implementation's limit of {MAX_TYPE_NESTING}"),
        ));
    }
    Ok(())
}

impl Parser {
    /// A `<value expression primary>` of arrays and rows that begins with
    /// an `<identifier>` no key word of SQL-92 names, if one is next:
    /// ARRAY `[`, ARRAY (, ROW (, CARDINALITY (, ARRAY_MAX_CARDINALITY (,
    /// TRIM_ARRAY ( or ARRAY_AGG (.
    pub(super) fn composite_primary(&mut self) -> Result<Option<ValueExpression>> {
        let Tok::Identifier(name) = self.peek() else { return Ok(None) };
        let at = self.here();
        let opens = |d: &str| matches!(self.peek_at(1), Tok::Delimiter(x) if *x == d);
        let composite = |c: Composite| Some(ValueExpression::Composite(Box::new(c), at));
        Ok(match name.as_str() {
            ARRAY_NAME if opens(delim::LEFT_BRACKET) => composite(Composite::Array(self.array_elements()?)),
            ARRAY_NAME if opens(delim::LEFT_PAREN) => composite(self.array_query()?),
            ROW_NAME if opens(delim::LEFT_PAREN) => {
                self.advance();
                composite(Composite::Row(self.parenthesized_list()?))
            }
            CARDINALITY | ARRAY_MAX_CARDINALITY | TRIM_ARRAY if opens(delim::LEFT_PAREN) => Some(self.array_function()?),
            ARRAY_AGG if opens(delim::LEFT_PAREN) => Some(ValueExpression::SetFunction(self.array_aggregate()?)),
            _ => None,
        })
    }

    /// [ISO/IEC 9075-2:2023 §6.5] Whether an `<empty specification>` of an
    /// array, ARRAY `[` `]`, is next, read if it is.
    pub(super) fn empty_specification(&mut self) -> bool {
        let array = matches!(self.peek(), Tok::Identifier(n) if n == ARRAY_NAME);
        let bracket = |p: &Self, k: usize, d: &str| matches!(p.peek_at(k), Tok::Delimiter(x) if *x == d);
        if !(array && bracket(self, 1, delim::LEFT_BRACKET) && bracket(self, 2, delim::RIGHT_BRACKET)) {
            return false;
        }
        for _ in 0..EMPTY_SPECIFICATION_TOKENS {
            self.advance();
        }
        true
    }

    /// [ISO/IEC 9075-2:2023 §6.49] `<array value constructor by
    /// enumeration>`'s elements, after ARRAY: none of an `<empty
    /// specification>` [§6.5].
    fn array_elements(&mut self) -> Result<Vec<ValueExpression>> {
        self.advance();
        self.expect_delim(delim::LEFT_BRACKET)?;
        let mut elements = Vec::new();
        if self.accept_delim(delim::RIGHT_BRACKET) {
            return Ok(elements);
        }
        loop {
            elements.push(self.nested(Parser::value_expression)?);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_BRACKET)?;
        Ok(elements)
    }

    /// [ISO/IEC 9075-2:2023 §6.49] `<array value constructor by query>` ::=
    /// ARRAY `<table subquery>`, its `<query expression>` with the `<order
    /// by clause>` it may end with [§7.17], which orders the elements
    /// \[NOTE 207\].
    fn array_query(&mut self) -> Result<Composite> {
        self.advance();
        let at = self.here();
        self.expect_delim(delim::LEFT_PAREN)?;
        let query = self.nested(Parser::query_expression)?;
        let mut order_by = Vec::new();
        if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            loop {
                order_by.push(self.sort_specification()?);
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Composite::ArrayQuery(Box::new(Subquery { query, at }), order_by))
    }

    /// [ISO/IEC 9075-2:2023 §7.1] ( `<row value constructor element>` ,
    /// `<row value constructor element list>` ), after its first element,
    /// before the comma: a `<row value constructor>` of two values at least.
    pub(super) fn row_constructor(&mut self, first: ValueExpression, at: usize) -> Result<ValueExpression> {
        let mut values = vec![first];
        while self.accept_delim(delim::COMMA) {
            values.push(self.nested(Parser::value_expression)?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Composite(Box::new(Composite::Row(values)), at))
    }

    /// ( `<value expression>` \[, ...\] ): one value at least.
    fn parenthesized_list(&mut self) -> Result<Vec<ValueExpression>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut values = Vec::new();
        loop {
            values.push(self.nested(Parser::value_expression)?);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(values)
    }

    /// [ISO/IEC 9075-2:2023 §6.31] `<cardinality expression>` ::=
    /// CARDINALITY ( `<collection value expression>` ), `<max cardinality
    /// expression>` ::= ARRAY_MAX_CARDINALITY ( `<array value expression>`
    /// ); [§6.48] `<trim array function>` ::= TRIM_ARRAY ( `<array value
    /// expression>` , `<numeric value expression>` ).
    fn array_function(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let named = |f: &&str| matches!(self.peek(), Tok::Identifier(n) if n == f);
        let name = [TRIM_ARRAY, ARRAY_MAX_CARDINALITY].into_iter().find(named).unwrap_or(CARDINALITY);
        self.advance();
        let args = self.parenthesized_list()?;
        let wanted = if name == TRIM_ARRAY { 2 } else { 1 };
        if args.len() != wanted {
            return Err(SqlError::at(Code::Syntax, at, format!("[ISO/IEC 9075-2:2023 §6.31, §6.48] {name} takes {wanted} operands")));
        }
        Ok(ValueExpression::Function { name, args, at })
    }

    /// [ISO/IEC 9075-2:2023 §10.9] `<array aggregate function>` ::=
    /// ARRAY_AGG ( `<value expression>` \[ORDER BY `<sort specification
    /// list>`\] ), each `<sort key>` a `<value expression>` \[ASC | DESC\].
    fn array_aggregate(&mut self) -> Result<SetFunctionSpecification> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let value = self.nested(Parser::value_expression)?;
        let mut order_by = Vec::new();
        if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            order_by = self.sort_specification_list()?;
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(SetFunctionSpecification { name: ARRAY_AGG, arg: SetArg::All(Box::new(value)), order_by, listagg: None, at })
    }

    /// `primary` followed by each `<array element reference>` `[` `<numeric
    /// value expression>` `]` [ISO/IEC 9075-2:2023 §6.25] and `<field
    /// reference>` . `<field name>` [§6.16] that follows it, in order.
    pub(super) fn postfixed(&mut self, mut primary: ValueExpression) -> Result<ValueExpression> {
        loop {
            let at = self.here();
            let part = if self.accept_delim(delim::LEFT_BRACKET) {
                let index = self.nested(Parser::value_expression)?;
                self.expect_delim(delim::RIGHT_BRACKET)?;
                Composite::Element(Box::new(primary), Box::new(index))
            } else if self.is_delim(delim::PERIOD) && matches!(self.peek_at(1), Tok::Identifier(_)) {
                self.advance();
                Composite::Field(Box::new(primary), self.identifier()?)
            } else {
                return Ok(primary);
            };
            self.deepen()?;
            primary = ValueExpression::Composite(Box::new(part), at);
        }
    }
}

impl Parser {
    /// [ISO/IEC 9075-2:2023 §7.16] After `<asterisked identifier chain>` .
    /// *: a `<qualified asterisk>` of `qualifier`, or with AS ( `<all fields
    /// column name list>` ) [Feature T053] an `<all fields reference>` of the
    /// column it names.
    pub(super) fn all_fields_names(&mut self, qualifier: TableName) -> Result<SelectSublist> {
        if !(self.is_kw(kw::AS) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))) {
            return Ok(SelectSublist::All(qualifier));
        }
        self.advance();
        let at = qualifier.at;
        let column = ColumnSpecification {
            qualifier: qualifier.auth.map(|auth| TableName { auth: None, ident: auth, at }),
            column: qualifier.ident,
            catalog: None,
            at,
        };
        let names = Some(self.name_list()?);
        Ok(SelectSublist::Fields { value: ValueExpression::Column(column), names, at })
    }

    /// [ISO/IEC 9075-2:2023 §7.16] `<all fields reference>` ::= `<value
    /// expression primary>` . * \[AS ( `<all fields column name list>` )\],
    /// after `value`, before the period.
    pub(super) fn all_fields(&mut self, value: ValueExpression, at: usize) -> Result<SelectSublist> {
        self.expect_delim(delim::PERIOD)?;
        self.expect_delim(delim::ASTERISK)?;
        let names = if self.accept_kw(kw::AS) { Some(self.name_list()?) } else { None };
        Ok(SelectSublist::Fields { value, names, at })
    }

    /// Whether a `<collection derived table>` begins here: UNNEST, no key
    /// word of SQL-92, before (.
    pub(super) fn at_unnest(&self) -> bool {
        self.is_word(UNNEST) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
    }

    /// [ISO/IEC 9075-2:2023 §7.6] `<collection derived table>` ::= UNNEST (
    /// `<collection value expression>` \[, ...\] ) \[WITH ORDINALITY\], then
    /// \[AS\] `<correlation name>` \[( `<derived column list>` )\], the
    /// correlation name not optional \[SR8\].
    pub(super) fn collection_derived_table(&mut self) -> Result<TableReference> {
        let at = self.here();
        self.advance();
        let collections = self.parenthesized_list()?;
        let ordinality = self.is_kw(kw::WITH) && matches!(self.peek_at(1), Tok::Identifier(w) if w == ORDINALITY);
        if ordinality {
            self.advance();
            self.advance();
        }
        let correlation = self
            .correlation()?
            .ok_or_else(|| self.syntax("[ISO/IEC 9075-2:2023 §7.6 F] a <collection derived table> has a correlation name"))?;
        let columns = self.derived_columns()?;
        Ok(TableReference::Unnest(UnnestTable { collections, ordinality, correlation, columns, at }))
    }
}
