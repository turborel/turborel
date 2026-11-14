//! The primaries of a `<value expression>` [X3.135-1986 §5.9; ISO/IEC
//! 9075:1992 §6.11]: value specifications, columns, set functions [§6.5],
//! CASE [§6.9], CAST [§6.10], scalar subqueries [§7.11], the string
//! functions [§6.6, §6.7], EXTRACT [§6.6], the datetime value functions
//! [§6.8], VALUE of a domain constraint [§6.2], NEXT VALUE FOR [ISO/IEC
//! 9075-2:2023 §6.14], and the \[EXTENSION\] function call.

use super::Parser;
use super::grouping::GROUPING;
use crate::ast::{
    CaseExpression, Predicate, SearchCondition, SetArg, SetFunctionSpecification, ValueExpression, ValueSpecification, WhenOperand,
};
use crate::error::Result;
use crate::functions::{ANY_VALUE, EVERY, FORMAT, GREATEST, LEAST, LISTAGG, LPAD, REGEX_FUNCTIONS, RPAD, TRIMS_AND_PADS, using};
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// A `<primary>` in parentheses: a `<scalar subquery>`, a `<row value
    /// constructor>` of two or more elements [ISO/IEC 9075-2:2023 §7.1],
    /// or a `<value expression>` nested.
    fn parenthesized_primary(&mut self) -> Result<ValueExpression> {
        if self.at_subquery() {
            return Ok(ValueExpression::Subquery(Box::new(self.subquery()?)));
        }
        let at = self.here();
        self.advance();
        let e = self.value_expression()?;
        if self.is_delim(delim::COMMA) {
            return self.row_constructor(e, at);
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
    pub(super) fn primary(&mut self) -> Result<ValueExpression> {
        match self.peek() {
            Tok::Delimiter(delim::LEFT_PAREN) => self.parenthesized_primary(),
            Tok::Identifier(_) => self.identifier_primary(),
            Tok::KeyWord(kw::COUNT | kw::AVG | kw::MAX | kw::MIN | kw::SUM | kw::ANY | kw::SOME) => {
                Ok(ValueExpression::SetFunction(self.set_function_specification()?))
            }
            Tok::KeyWord(kw::CASE | kw::NULLIF | kw::COALESCE) => Ok(ValueExpression::Case(Box::new(self.case_expression()?))),
            Tok::KeyWord(kw::CAST) => self.cast_specification(),
            Tok::KeyWord(kw::COLLATION) => self.collation_for(),
            Tok::KeyWord(kw::CURRENT_DATE | kw::CURRENT_TIME | kw::CURRENT_TIMESTAMP) => {
                let f = self.datetime_value_function()?.ok_or_else(|| self.syntax("expected a <datetime value function>"))?;
                Ok(ValueExpression::Datetime(f))
            }
            Tok::KeyWord(kw::EXTRACT) => self.extract_expression(),
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
            Tok::Introducer(_) => {
                if matches!(self.peek_at(1), Tok::CharacterString(_)) {
                    Ok(ValueExpression::Value(self.value_specification()?))
                } else {
                    Ok(ValueExpression::Column(self.column_specification()?))
                }
            }
            // [ISO/IEC 9075:1992 §5.4] MODULE.T.C: a column of a declared
            // local temporary table.
            Tok::KeyWord(kw::MODULE) => Ok(ValueExpression::Column(self.column_specification()?)),
            Tok::KeyWord(kw::VALUE) => self.value_reference(),
            Tok::KeyWord(kw::NEXT) => self.next_value_expression(),
            Tok::KeyWord(_) => self.string_function(),
            Tok::Delimiter(_) | Tok::End => Err(self.syntax("expected a <value expression>")),
        }
    }

    /// A `<primary>` that starts with an `<identifier>`: a function call, a
    /// `<parameter specification>`, or a `<column specification>`.
    fn identifier_primary(&mut self) -> Result<ValueExpression> {
        if let Some(composite) = self.composite_primary()? {
            return Ok(composite);
        }
        // [ISO/IEC 9075-2:2023 §10.9] EVERY, no key word of SQL-92, before
        // "(" the set function.
        // [ISO/IEC 9075-2:2023 §10.9] EVERY and ANY_VALUE, no key words of
        // SQL-92, before "(" set functions, and LISTAGG the <listagg set
        // function>.
        let opens = matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN));
        if opens && (self.is_word(EVERY) || self.is_word(ANY_VALUE)) {
            return Ok(ValueExpression::SetFunction(self.set_function_specification()?));
        }
        if opens && self.is_word(LISTAGG) {
            return Ok(ValueExpression::SetFunction(self.listagg()?));
        }
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
        // [ISO/IEC 9075-2:2023 §6.9] GROUPING, no key word of SQL-92,
        // before "(" the <grouping operation>.
        if self.is_word(GROUPING) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return Ok(ValueExpression::SetFunction(self.grouping_operation()?));
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
        let qualified = matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD))
            && matches!(self.peek_at(2), Tok::Identifier(_))
            && matches!(self.peek_at(3), Tok::Delimiter(delim::LEFT_PAREN));
        if qualified || matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return self.function_call();
        }
        if self.at_parameter() {
            return Ok(ValueExpression::Value(ValueSpecification::Parameter(self.parameter_specification()?)));
        }
        self.identifier_chain()
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
            | Tok::Delimiter(_)
            | Tok::End => return Err(self.syntax("expected a <set function specification>")),
        };
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let arg = if name == kw::COUNT && self.accept_delim(delim::ASTERISK) {
            SetArg::CountStar
        } else if self.accept_kw(kw::DISTINCT) {
            SetArg::Distinct(Box::new(self.value_expression()?))
        } else {
            // [SR1] ALL, written or implicit.
            self.accept_kw(kw::ALL);
            SetArg::All(Box::new(self.value_expression()?))
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(SetFunctionSpecification { name, arg, order_by: Vec::new(), listagg: None, at })
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
        Ok(SetFunctionSpecification { name: GROUPING, arg: SetArg::Grouping(columns), order_by: Vec::new(), listagg: None, at })
    }

    /// [ISO/IEC 9075:1992 §6.9] `<case expression>`: NULLIF, COALESCE, or
    /// CASE ... END, simple or searched.
    fn case_expression(&mut self) -> Result<CaseExpression> {
        if self.accept_kw(kw::NULLIF) {
            self.expect_delim(delim::LEFT_PAREN)?;
            let first = self.value_expression()?;
            self.expect_delim(delim::COMMA)?;
            let second = self.value_expression()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(CaseExpression::NullIf(first, second));
        }
        if self.accept_kw(kw::COALESCE) {
            // COALESCE (<value expression> { , <value expression> }...): two at least.
            self.expect_delim(delim::LEFT_PAREN)?;
            let mut values = vec![self.value_expression()?];
            self.expect_delim(delim::COMMA)?;
            values.push(self.value_expression()?);
            while self.accept_delim(delim::COMMA) {
                values.push(self.value_expression()?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(CaseExpression::Coalesce(values));
        }
        self.expect_kw(kw::CASE)?;
        let case = if self.is_kw(kw::WHEN) {
            let mut whens = Vec::new();
            while self.accept_kw(kw::WHEN) {
                let condition = self.search_condition()?;
                self.expect_kw(kw::THEN)?;
                whens.push((condition, self.result()?));
            }
            CaseExpression::Searched { whens, otherwise: self.else_clause()? }
        } else {
            let operand = self.case_operand()?;
            let mut whens = Vec::new();
            self.expect_kw(kw::WHEN)?;
            loop {
                // [ISO/IEC 9075-2:2023 §6.12] `<when operand list>`: one
                // `<when operand>` or more, separated by commas.
                let mut list = vec![self.when_operand(&operand)?];
                while self.accept_delim(delim::COMMA) {
                    list.push(self.when_operand(&operand)?);
                }
                self.expect_kw(kw::THEN)?;
                whens.push((list, self.result()?));
                if !self.accept_kw(kw::WHEN) {
                    break;
                }
            }
            overlapping_alike(&whens).map_err(|m| self.rule(m))?;
            CaseExpression::Simple { operand, whens, otherwise: self.else_clause()? }
        };
        self.expect_kw(kw::END)?;
        Ok(case)
    }

    /// `<result>` ::= `<result expression>` | NULL: none for NULL.
    fn result(&mut self) -> Result<Option<ValueExpression>> {
        if self.accept_kw(kw::NULL) { Ok(None) } else { self.value_expression().map(Some) }
    }

    /// \[`<else clause>`\] ::= ELSE `<result>`: none if NULL or not written.
    fn else_clause(&mut self) -> Result<Option<ValueExpression>> {
        if self.accept_kw(kw::ELSE) { self.result() } else { Ok(None) }
    }

    /// [ISO/IEC 9075-2:2023 §6.13] `<cast specification>` ::= CAST
    /// (`<cast operand>` AS `<cast target>` \[FORMAT `<cast template>`\]),
    /// the operand a `<value expression>` or NULL, the target a `<data
    /// type>` or a `<domain name>`, the template a `<character string
    /// literal>`.
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
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Cast { operand, target, format, at })
    }

    /// [ISO/IEC 9075:1992 §6.6, §6.7] A string function, called with its
    /// operands in order (`functions/strings.rs`): SUBSTRING, UPPER,
    /// LOWER, TRIM, POSITION, CHAR_LENGTH or CHARACTER_LENGTH,
    /// OCTET_LENGTH, BIT_LENGTH.
    ///
    /// Read and refused: CONVERT and TRANSLATE, of character sets
    /// (`Parser::conversion`).
    fn string_function(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let Tok::KeyWord(word) = *self.peek() else { return Err(self.syntax("expected a <value expression>")) };
        let name = match word {
            kw::SUBSTRING | kw::UPPER | kw::LOWER | kw::TRIM | kw::POSITION | kw::OCTET_LENGTH | kw::BIT_LENGTH => word,
            kw::CHAR_LENGTH | kw::CHARACTER_LENGTH => kw::CHAR_LENGTH,
            kw::CONVERT | kw::TRANSLATE => return self.conversion(),
            _ => return Err(self.syntax("expected a <value expression>")),
        };
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let args = match name {
            kw::SUBSTRING => self.substring_operands()?,
            kw::TRIM => self.trim_operands()?,
            kw::POSITION => {
                // POSITION (<character value expression> IN <character value expression>)
                let needle = self.common_value_expression()?;
                self.expect_kw(kw::IN)?;
                vec![needle, self.common_value_expression()?]
            }
            _ => vec![self.common_value_expression()?],
        };
        // [ISO/IEC 9075-2:2023 §6.31, §6.33] USING of CHAR_LENGTH, POSITION
        // and SUBSTRING [Feature T062].
        let name = match self.char_length_units()? {
            Some(units) => using(name, units).ok_or_else(|| self.syntax("USING follows no CHAR_LENGTH, POSITION or SUBSTRING"))?,
            None => name,
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Function { name, args, at })
    }

    /// SUBSTRING (`<character value expression>` FROM `<start position>`
    /// \[FOR `<string length>`\]): its operands.
    fn substring_operands(&mut self) -> Result<Vec<ValueExpression>> {
        let source = self.common_value_expression()?;
        self.expect_kw(kw::FROM)?;
        let mut args = vec![source, self.common_value_expression()?];
        if self.accept_kw(kw::FOR) {
            args.push(self.common_value_expression()?);
        }
        Ok(args)
    }

    /// TRIM (\[\[`<trim specification>`\] \[`<trim character>`\] FROM\]
    /// `<trim source>`): the specification, BOTH if none [SR6b], as a
    /// string; the character, ' ' if none [SR6c]; and the source.
    fn trim_operands(&mut self) -> Result<Vec<ValueExpression>> {
        let literal = |s: &str| ValueExpression::Value(ValueSpecification::Literal(crate::ast::Literal::Character(s.to_owned())));
        let spec = [kw::LEADING, kw::TRAILING, kw::BOTH].into_iter().find(|w| self.is_kw(w));
        if let Some(w) = spec {
            self.advance();
            let character = if self.accept_kw(kw::FROM) {
                literal(" ")
            } else {
                let c = self.common_value_expression()?;
                self.expect_kw(kw::FROM)?;
                c
            };
            return Ok(vec![literal(w), character, self.common_value_expression()?]);
        }
        // [§6.7] FROM with neither: BOTH [SR6b] and a space [SR6c].
        if self.accept_kw(kw::FROM) {
            return Ok(vec![literal(kw::BOTH), literal(" "), self.common_value_expression()?]);
        }
        let first = self.common_value_expression()?;
        if self.accept_kw(kw::FROM) {
            // [SR6a] FROM after a <trim character>.
            return Ok(vec![literal(kw::BOTH), first, self.common_value_expression()?]);
        }
        Ok(vec![literal(kw::BOTH), literal(" "), first])
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
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(ValueExpression::Call { schema, name, args, at, extension: self.extensions })
    }
}

/// A space, the trim character or padding none is written gives.
fn space() -> ValueExpression {
    ValueExpression::Value(ValueSpecification::Literal(crate::ast::Literal::Character(" ".to_owned())))
}

/// [ISO/IEC 9075-2:2023 §6.12 SR2c] Of a simple CASE's `whens`, each
/// `<when operand>` an `<overlaps predicate part 2>` or none is: an
/// `<overlaps predicate part 1>` is compared by OVERLAPS alone, and a
/// `<row value predicand>` never by it.
fn overlapping_alike(whens: &[(Vec<WhenOperand>, Option<ValueExpression>)]) -> std::result::Result<(), &'static str> {
    let overlaps = |w: &WhenOperand| match w {
        WhenOperand::Condition(SearchCondition::Predicate(p, _)) => matches!(**p, Predicate::Overlaps(..)),
        WhenOperand::Condition(_) | WhenOperand::Value(..) => false,
    };
    let (some, all) = whens.iter().flat_map(|(list, _)| list).fold((false, true), |(some, all), w| {
        let o = overlaps(w);
        (some || o, all && o)
    });
    if some && !all {
        return Err("[ISO/IEC 9075-2:2023 §6.12 SR2c] a simple CASE's <when operand>s are each OVERLAPS or none is");
    }
    Ok(())
}
