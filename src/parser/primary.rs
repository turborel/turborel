//! The primaries of a `<value expression>` [X3.135-1986 §5.9; ISO/IEC
//! 9075:1992 §6.11]: value specifications, columns, set functions [§6.5],
//! CASE [§6.9], CAST [§6.10], scalar subqueries [§7.11], the string
//! functions [§6.6, §6.7], EXTRACT [§6.6], the datetime value functions
//! [§6.8], VALUE of a domain constraint [§6.2], NEXT VALUE FOR [ISO/IEC
//! 9075-2:2023 §6.14], WIDTH_BUCKET [§6.31], the ordered set functions
//! [§10.9], the SQL/JSON functions [§6.28, §6.34, §6.35, §10.11] by
//! `json.rs`, the \[EXTENSION\] function call, and, by `pattern/navigation.rs`,
//! row pattern matching's expressions [§6.27, §6.31, §6.33].

mod case;
mod strings;

use super::Parser;
use super::grouping::GROUPING;
use crate::ast::{SetArg, SetFunctionSpecification, ValueExpression, ValueSpecification};
use crate::error::Result;
use crate::functions::{
    ANY_VALUE, EVERY, FORMAT, GREATEST, LEAST, LISTAGG, LPAD, OVERLAY, REGEX_FUNCTIONS, RPAD, TRIMS_AND_PADS, WIDTH_BUCKET,
};
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// A `<primary>` in parentheses: a `<scalar subquery>`, a `<row value
    /// constructor>` of two or more elements [ISO/IEC 9075-2:2023 §7.1],
    /// or a `<value expression>` nested.
    ///
    /// \[DESIGN\] What follows the first value is read by
    /// `parenthesized_rest`, and a subquery by `scalar_subquery`, so the
    /// frame a nesting of parentheses recurs through holds neither's
    /// locals (RULES.md R-CF-6).
    fn parenthesized_primary(&mut self) -> Result<ValueExpression> {
        if self.at_subquery() {
            return self.scalar_subquery();
        }
        let at = self.here();
        self.advance();
        let e = self.value_expression()?;
        self.parenthesized_rest(e, at)
    }

    /// [X3.135-1986 §5.9] A `<scalar subquery>` as a `<primary>`.
    fn scalar_subquery(&mut self) -> Result<ValueExpression> {
        Ok(ValueExpression::Subquery(Box::new(self.subquery()?)))
    }

    /// The rest of a `<primary>` in parentheses, begun at `at`, whose first
    /// value is `e`: the other elements of a `<row value constructor>`, or
    /// ")".
    fn parenthesized_rest(&mut self, e: ValueExpression, at: usize) -> Result<ValueExpression> {
        if self.is_delim(delim::COMMA) {
            return self.row_constructor(e, at);
        }
        // [ISO/IEC 9075-2:2023 §6.18] a <generalized invocation>.
        if self.is_kw(kw::AS) {
            return self.generalized_invocation(e, at);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Nested(Box::new(e)))
    }

    /// `<primary>` ::= `<value specification>` | `<column specification>`
    ///   | `<set function specification>` | (`<value expression>`), a
    ///   truth value among them [ISO/IEC 9075-2:2023 §6.3]
    ///   | `<case expression>` | `<cast specification>` | `<scalar
    ///   subquery>` | `<numeric value function>` | `<string value function>`
    ///   | `<datetime value function>` [ISO/IEC 9075:1992 §6.11]
    ///
    /// \[DESIGN\] Each arm here is one call, its result its own; a primary
    /// read and then wrapped is read by `wrapped_primary`. So the frame a
    /// nesting of expressions recurs through holds no arm's locals
    /// (RULES.md R-CF-6).
    pub(super) fn primary(&mut self) -> Result<ValueExpression> {
        // [GS §10] GRAPH PATH aggregates, and the variables of a PGQL or
        // Cypher query.
        match self.compat_primary()? {
            Some(compat) => Ok(compat),
            None => self.sql_primary(),
        }
    }

    /// A `<value expression primary>` of SQL alone, as `primary` reads it.
    fn sql_primary(&mut self) -> Result<ValueExpression> {
        match self.peek() {
            Tok::Delimiter(delim::LEFT_PAREN) => self.parenthesized_primary(),
            Tok::Identifier(_) => self.identifier_primary(),
            Tok::KeyWord(kw::CAST) => self.cast_specification(),
            Tok::KeyWord(kw::COLLATION) => self.collation_for(),
            Tok::KeyWord(kw::EXTRACT) => self.extract_expression(),
            Tok::KeyWord(kw::VALUE) => self.value_reference(),
            // [ISO/IEC 9075-2:2023 §6.27] NEXT, FIRST and LAST before "(".
            Tok::KeyWord(kw::NEXT) => self.next_primary(),
            Tok::KeyWord(kw::FIRST | kw::LAST) => self.first_primary(),
            Tok::KeyWord(
                kw::COUNT
                | kw::AVG
                | kw::MAX
                | kw::MIN
                | kw::SUM
                | kw::ANY
                | kw::SOME
                | kw::CASE
                | kw::NULLIF
                | kw::COALESCE
                | kw::CURRENT_DATE
                | kw::CURRENT_TIME
                | kw::CURRENT_TIMESTAMP
                | kw::DATE
                | kw::TIME
                | kw::TIMESTAMP
                | kw::INTERVAL
                | kw::TRUE
                | kw::FALSE
                | kw::UNKNOWN
                | kw::USER
                | kw::CURRENT_USER
                | kw::SESSION_USER
                | kw::SYSTEM_USER
                | kw::MODULE,
            )
            | Tok::Delimiter(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::End => self.wrapped_primary(),
            // [ISO/IEC 9075-2:2023 §6.51, §6.52] SET ( and TABLE (.
            Tok::KeyWord(kw::SET | kw::TABLE) => match self.keyword_multiset_primary()? {
                Some(multiset) => Ok(multiset),
                None => self.string_function(),
            },
            Tok::KeyWord(_) => self.key_word_call(),
        }
    }

    /// A primary that begins with any other key word: a string function,
    /// or \[EXTENSION\] after UNION or OVERLAPS (, which begins no `<value
    /// expression primary>` of SQL, the call of a function of that name,
    /// as Simple feature access's Union and Overlaps [OGC 06-104r4 §7.2.8]
    /// (RULES.md D136).
    fn key_word_call(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let named = matches!(self.peek(), Tok::KeyWord(kw::UNION | kw::OVERLAPS));
        let Tok::KeyWord(name) = self.peek() else { return self.string_function() };
        let name = (*name).to_owned();
        if !named || !matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return self.string_function();
        }
        self.advance();
        self.advance();
        let mut args = vec![self.common_value_expression()?];
        while self.accept_delim(delim::COMMA) {
            args.push(self.common_value_expression()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Call { schema: None, name, args, at, extension: self.extensions })
    }

    /// A `<primary>` read, then wrapped as one: a set function, a CASE, a
    /// datetime value function, a value specification or a column.
    fn wrapped_primary(&mut self) -> Result<ValueExpression> {
        match self.peek() {
            Tok::KeyWord(kw::COUNT | kw::AVG | kw::MAX | kw::MIN | kw::SUM | kw::ANY | kw::SOME) => {
                Ok(ValueExpression::SetFunction(self.set_function_specification()?))
            }
            Tok::KeyWord(kw::CASE | kw::NULLIF | kw::COALESCE) => Ok(ValueExpression::Case(Box::new(self.case_expression()?))),
            Tok::KeyWord(kw::CURRENT_DATE | kw::CURRENT_TIME | kw::CURRENT_TIMESTAMP) => {
                let f = self.datetime_value_function()?.ok_or_else(|| self.syntax("expected a <datetime value function>"))?;
                Ok(ValueExpression::Datetime(f))
            }
            Tok::KeyWord(kw::DATE | kw::TIME | kw::TIMESTAMP | kw::INTERVAL)
            | Tok::KeyWord(kw::TRUE | kw::FALSE | kw::UNKNOWN)
            | Tok::KeyWord(kw::USER)
            | Tok::KeyWord(kw::CURRENT_USER)
            | Tok::KeyWord(kw::SESSION_USER)
            | Tok::KeyWord(kw::SYSTEM_USER)
            | Tok::Delimiter(delim::COLON)
            | Tok::Delimiter(delim::QUESTION)
            | Tok::Delimiter(delim::PLUS)
            | Tok::Delimiter(delim::MINUS)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_) => Ok(ValueExpression::Value(self.value_specification()?)),
            // [ISO/IEC 9075:1992 §5.3, §5.4] a character string literal, or
            // a column, of a character set.
            Tok::Introducer(_) | Tok::LargeObjectLength(_) => {
                if matches!(self.peek_at(1), Tok::CharacterString(_)) {
                    Ok(ValueExpression::Value(self.value_specification()?))
                } else {
                    Ok(ValueExpression::Column(self.column_specification()?))
                }
            }
            // [ISO/IEC 9075:1992 §5.4] MODULE.T.C: a column of a declared
            // local temporary table.
            Tok::KeyWord(kw::MODULE) => Ok(ValueExpression::Column(self.column_specification()?)),
            // Of the others, as `primary` reads them.
            Tok::Delimiter(delim::LEFT_PAREN) | Tok::Identifier(_) | Tok::KeyWord(_) => self.primary(),
            Tok::Delimiter(_) | Tok::End => Err(self.syntax("expected a <value expression>")),
        }
    }

    /// A `<primary>` that starts with an `<identifier>`: a function call, a
    /// `<parameter specification>`, or a `<column specification>`.
    fn identifier_primary(&mut self) -> Result<ValueExpression> {
        // [ISO/IEC 9075-2:2023 §6.4] CURRENT_ROLE standing alone.
        if self.at_current_role() {
            return Ok(ValueExpression::Value(self.value_specification()?));
        }
        if let Some(special) = self.composite_pattern_or_aggregate()? {
            return Ok(special);
        }
        let opens = matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN));
        // [ISO/IEC 9075-2:2023 §6.15] GREATEST and LEAST, no key words of
        // SQL-92, before "(" the functions.
        let extreme = if let Tok::Identifier(name) = self.peek() { [GREATEST, LEAST].into_iter().find(|f| name == f) } else { None };
        if let Some(name) = extreme.filter(|_| matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))) {
            return self.greatest_or_least(name);
        }
        // [ISO/IEC 9075-2:2023 §6.33] LTRIM, RTRIM, BTRIM, LPAD and RPAD,
        // no key words of SQL-92, before "(" the functions.
        let trim = if let Tok::Identifier(name) = self.peek() { TRIMS_AND_PADS.into_iter().find(|f| name == f) } else { None };
        if let Some(name) = trim.filter(|_| matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))) {
            return self.trim_or_pad(name);
        }
        // [ISO/IEC 9075-2:2023 §6.31, §6.33] OCCURRENCES_REGEX,
        // POSITION_REGEX, SUBSTRING_REGEX and TRANSLATE_REGEX, no key words
        // of SQL-92, before "(" the functions.
        let regex = if let Tok::Identifier(name) = self.peek() { REGEX_FUNCTIONS.into_iter().find(|f| name == f) } else { None };
        if let Some(name) = regex.filter(|_| matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))) {
            return self.regex_function(name);
        }
        // [ISO/IEC 9075-2:2023 §6.33] OVERLAY, no key word of SQL-92,
        // before "(" the <character overlay function> or <binary overlay
        // function>.
        if opens && self.is_word(OVERLAY) {
            return self.overlay();
        }
        // [ISO/IEC 9075-2:2023 §6.31] WIDTH_BUCKET, no key word of SQL-92,
        // before "(" the <width bucket function>.
        if opens && self.is_word(WIDTH_BUCKET) {
            return self.width_bucket();
        }
        // [ISO/IEC 9075-2:2023 §6.9] GROUPING, no key word of SQL-92,
        // before "(" the <grouping operation>.
        if self.is_word(GROUPING) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return Ok(ValueExpression::SetFunction(self.grouping_operation()?));
        }
        // [ISO/IEC 9075-2:2023 §6.11] ROW_NUMBER of a row marker and
        // VALUE_OF at one, no key words of SQL-92.
        if let Some(nesting) = self.at_nested_window() {
            return self.nested_window(nesting);
        }
        // [ISO/IEC 9075-2:2023 §10.9] the ordered set functions, of names
        // no key words of SQL-92.
        if let Some(ordered) = self.ordered_set_function() {
            return ordered;
        }
        // [ISO/IEC 9075-2:2023 §6.10] ROW_NUMBER and the rank functions,
        // no key words of SQL-92, before ( ) OVER.
        if let Some(kind) = self.at_rank_function() {
            return self.rank_function(kind);
        }
        // [ISO/IEC 9075-2:2023 §6.10] NTILE, LEAD, LAG, FIRST_VALUE,
        // LAST_VALUE and NTH_VALUE, no key words of SQL-92, before OVER.
        if let Some(named) = self.at_value_function() {
            return self.value_function(named);
        }
        // [ISO/IEC 9075-2:2023 §10.4] a <routine invocation>: its name,
        // perhaps of a schema, then "(".
        // [ISO/IEC 9075-2:2023 §6.40] SPECIFICTYPE, no routine's name.
        let qualified = matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD))
            && matches!(self.peek_at(2), Tok::Identifier(w) if w != super::structured::SPECIFICTYPE)
            && matches!(self.peek_at(3), Tok::Delimiter(delim::LEFT_PAREN));
        if qualified || matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return self.function_call();
        }
        if self.at_parameter() {
            return Ok(ValueExpression::Value(ValueSpecification::Parameter(self.parameter_specification()?)));
        }
        self.identifier_chain()
    }

    /// A `<primary>` that starts with an `<identifier>` and is an array or
    /// row expression [ISO/IEC 9075-2:2023 §6.47, §7.1], one of
    /// user-defined types [§6.19, §6.20, §6.23, §6.24], an expression of
    /// row pattern matching [§6.9, §6.10, §6.27, §6.31, §6.33], or an
    /// aggregate of a name no key word of SQL-92 [§10.9], or an SQL/JSON
    /// function [§6.28, §6.34, §6.35, §10.11], if one is next.
    fn composite_pattern_or_aggregate(&mut self) -> Result<Option<ValueExpression>> {
        // [EXTENSION] UUID '...', a UUID's literal, CAST of its string to
        // UUID (RULES.md D181).
        if self.is_word(crate::value::types::uuid::UUID_NAME) && matches!(self.peek_at(1), Tok::CharacterString(_)) {
            return self.uuid_literal().map(Some);
        }
        // [EXTENSION] [P1 §4.7] a graph function, or a graph predicate as
        // a value, inside a GRAPH_TABLE.
        if let Some(graph) = self.graph_primary()? {
            return Ok(Some(graph));
        }
        // [ISO/IEC 9075-2:2023 §6.31, §10.9] Feature T621's functions.
        if let Some(numeric) = self.numeric_or_statistical() {
            return numeric.map(Some);
        }
        if let Some(composite) = self.composite_primary()? {
            return Ok(Some(composite));
        }
        if let Some(pattern) = self.row_pattern_primary_expression()? {
            return Ok(Some(pattern));
        }
        // [ISO/IEC 9075-2:2023 §6.4, §6.19, §6.20, §6.23, §6.24] NEW, TREAT,
        // DEREF, T::M and the transform groups, no key words of SQL-92.
        if let Some(structured) = self.structured_primary()? {
            return Ok(Some(structured));
        }
        // [ISO/IEC 9075-2:2023 §6.28, §6.34, §6.35, §10.11] the SQL/JSON
        // functions, no key words of SQL-92, before "(" the functions.
        if let Some(json) = self.json_primary()? {
            return Ok(Some(json));
        }
        if let Some(xml) = self.xml_primary()? {
            return Ok(Some(xml));
        }
        // [ISO/IEC 9075-2:2023 §10.9] EVERY and ANY_VALUE, no key words of
        // SQL-92, before "(" set functions, and LISTAGG the <listagg set
        // function>.
        let opens = matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN));
        if opens && (self.is_word(EVERY) || self.is_word(ANY_VALUE)) {
            return Ok(Some(ValueExpression::SetFunction(self.set_function_specification()?)));
        }
        if opens && self.is_word(LISTAGG) {
            return Ok(Some(ValueExpression::SetFunction(self.listagg()?)));
        }
        Ok(None)
    }

    /// [ISO/IEC 9075:1992 §6.5] `<set function specification>` ::= COUNT(*)
    /// | `<set function type>` (\[DISTINCT | ALL\] `<value expression>`)
    fn set_function_specification(&mut self) -> Result<SetFunctionSpecification> {
        let at = self.here();
        let name = match self.peek() {
            Tok::KeyWord(name) => *name,
            // [ISO/IEC 9075-2:2023 §10.9] EVERY and ANY_VALUE, no key words
            // of SQL-92.
            Tok::Identifier(name) if name == EVERY => EVERY,
            Tok::Identifier(name) if name == ANY_VALUE => ANY_VALUE,
            Tok::Identifier(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => return Err(self.syntax("expected a <set function specification>")),
        };
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let counted = if name == kw::COUNT { self.row_pattern_count_function()? } else { None };
        let arg = if let Some(variable) = counted {
            variable
        } else if name == kw::COUNT && self.accept_delim(delim::ASTERISK) {
            SetArg::CountStar
        } else if self.accept_kw(kw::DISTINCT) {
            SetArg::Distinct(Box::new(self.value_expression()?))
        } else {
            // [SR1] ALL, written or implicit.
            self.accept_kw(kw::ALL);
            SetArg::All(Box::new(self.value_expression()?))
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(SetFunctionSpecification {
            name,
            arg,
            order_by: Vec::new(),
            listagg: None,
            json: None,
            filter: None,
            semantics: None,
            returning: None,
            at,
        })
    }

    /// [ISO/IEC 9075-2:2023 §6.9] `<grouping operation>` ::= GROUPING (
    /// `<column reference>` \[{, `<column reference>`}...\] ).
    fn grouping_operation(&mut self) -> Result<SetFunctionSpecification> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut columns = vec![ValueExpression::Column(self.column_specification()?)];
        while self.accept_delim(delim::COMMA) {
            columns.push(ValueExpression::Column(self.column_specification()?));
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(SetFunctionSpecification {
            name: GROUPING,
            arg: SetArg::Grouping(columns),
            order_by: Vec::new(),
            listagg: None,
            json: None,
            filter: None,
            semantics: None,
            returning: None,
            at,
        })
    }

    /// [ISO/IEC 9075-2:2023 §6.13] `<cast specification>` ::= CAST
    /// (`<cast operand>` AS `<cast target>` \[FORMAT `<cast template>`\]),
    /// the operand a `<value expression>` or NULL, the target a `<data
    /// type>` or a `<domain name>`, the template a `<character string
    /// literal>`; and [ISO/IEC 9075-14:2006 draft §6.5] \[`<XML passing
    /// mechanism>`\] before its ")".
    fn cast_specification(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_kw(kw::CAST)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let operand = if self.accept_kw(kw::NULL) { None } else { Some(Box::new(self.value_expression()?)) };
        self.expect_kw(kw::AS)?;
        let target = self.type_name()?;
        let format = if self.accept_word(FORMAT) {
            let Tok::CharacterString(template) = self.peek().clone() else {
                return Err(self.syntax("expected a <cast template>, a <character string literal>, after FORMAT"));
            };
            self.advance();
            Some(template)
        } else {
            None
        };
        let passing = self.passing_mechanism()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Cast { operand, target, format, passing, at })
    }

    /// \[EXTENSION\] `UUID <character string literal>`: CAST of the string
    /// to UUID (RULES.md D181).
    fn uuid_literal(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let target = self.type_name()?;
        let operand = Some(Box::new(ValueExpression::Value(self.value_specification()?)));
        Ok(ValueExpression::Cast { operand, target, format: None, passing: None, at })
    }

    /// [ISO/IEC 9075-2:2023 §6.15] `<greatest or least function>` ::=
    /// `<greatest or least>` ( `<value expression>` { , `<value
    /// expression>` }... ): two values at least, `name` the function.
    fn greatest_or_least(&mut self, name: &'static str) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut args = vec![self.value_expression()?];
        self.expect_delim(delim::COMMA)?;
        args.push(self.value_expression()?);
        while self.accept_delim(delim::COMMA) {
            args.push(self.value_expression()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Function { name, args, at })
    }

    /// [ISO/IEC 9075-2:2023 §6.33] `<multi-character trim function>` ::=
    /// { BTRIM | LTRIM | RTRIM } ( `<trim source>` \[ , `<trim character>`
    /// \] ), the character ' ' if none is written \[SR15b.ii\]; or `<pad
    /// function>` ::= { LPAD | RPAD } ( `<pad source>` , `<total pad
    /// length>` \[ , `<padding character value expression>` \] ), the
    /// padding ' ' if none is written \[SR16a\]: `name` the function.
    fn trim_or_pad(&mut self, name: &'static str) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut args = vec![self.common_value_expression()?];
        if name == LPAD || name == RPAD {
            self.expect_delim(delim::COMMA)?;
            args.push(self.common_value_expression()?);
        }
        args.push(if self.accept_delim(delim::COMMA) { self.common_value_expression()? } else { space() });
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Function { name, args, at })
    }

    /// [ISO/IEC 9075-2:2023 §6.31] `<width bucket function>` ::=
    /// WIDTH_BUCKET ( `<width bucket operand>` , `<width bucket bound 1>` ,
    /// `<width bucket bound 2>` , `<width bucket count>` ), each a
    /// `<numeric value expression>`, called with them in order
    /// (`functions/width_bucket.rs`).
    fn width_bucket(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut args = vec![self.common_value_expression()?];
        for _ in 1..WIDTH_BUCKET_OPERANDS {
            self.expect_delim(delim::COMMA)?;
            args.push(self.common_value_expression()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Function { name: WIDTH_BUCKET, args, at })
    }

    /// [ISO/IEC 9075-2:2023 §10.4] `<routine invocation>` ::= `<routine
    /// name>` `<SQL argument list>`, the name perhaps of a schema; or
    /// \[EXTENSION\] the call of a function registered, which the binder
    /// takes while SQL.FUNCTION_CALLS is ON.
    fn function_call(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let mut name = self.identifier()?;
        let mut schema = None;
        if self.accept_delim(delim::PERIOD) {
            schema = Some(std::mem::replace(&mut name, self.identifier()?));
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut args = Vec::new();
        if !self.accept_delim(delim::RIGHT_PAREN) {
            args.push(self.common_value_expression()?);
            while self.accept_delim(delim::COMMA) {
                args.push(self.common_value_expression()?);
            }
            // [EXTENSION] FROM_VECTOR's clauses (RULES.md D93).
            if schema.is_none() && name == super::vectors::FROM_VECTOR {
                self.vector_text_clauses(&mut name, &mut args)?;
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(ValueExpression::Call { schema, name, args, at, extension: self.extensions })
    }
}

/// [ISO/IEC 9075-2:2023 §6.31] The operands of a `<width bucket
/// function>`.
const WIDTH_BUCKET_OPERANDS: usize = 4;

/// A space, the trim character or padding none is written gives.
fn space() -> ValueExpression {
    ValueExpression::Value(ValueSpecification::Literal(crate::ast::Literal::Character(" ".to_owned())))
}
