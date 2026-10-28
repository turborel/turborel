//! The primaries of a `<value expression>` [X3.135-1986 §5.9; ISO/IEC
//! 9075:1992 §6.11]: value specifications, columns, set functions
//! [§6.5], CASE [§6.9], CAST [§6.10], scalar subqueries [§7.11], the string
//! functions [§6.6, §6.7], and the \[EXTENSION\] function call. Split from
//! parser/expression.rs (RULES.md R-SIZE-2); the methods extend the same
//! `Parser`.

use super::Parser;
use crate::ast::{CaseExpression, SetArg, SetFunctionSpecification, ValueExpression, ValueSpecification};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// `<primary>` ::= `<value specification>` | `<column specification>`
    ///   | `<set function specification>` | (`<value expression>`)
    ///   | `<case expression>` | `<cast specification>` | `<scalar
    ///   subquery>` | `<numeric value function>` | `<string value function>`
    ///   [ISO/IEC 9075:1992 §6.11]
    pub(super) fn primary(&mut self) -> Result<ValueExpression> {
        match self.peek() {
            Tok::Delimiter(delim::LEFT_PAREN) => {
                if self.at_subquery() {
                    return Ok(ValueExpression::Subquery(Box::new(self.subquery()?)));
                }
                self.advance();
                let e = self.value_expression()?;
                self.expect_delim(delim::RIGHT_PAREN)?;
                Ok(ValueExpression::Nested(Box::new(e)))
            }
            Tok::Identifier(_) => self.identifier_primary(),
            Tok::KeyWord(kw::COUNT) | Tok::KeyWord(kw::AVG) | Tok::KeyWord(kw::MAX) | Tok::KeyWord(kw::MIN) | Tok::KeyWord(kw::SUM) => {
                Ok(ValueExpression::SetFunction(self.set_function_specification()?))
            }
            Tok::KeyWord(kw::CASE) | Tok::KeyWord(kw::NULLIF) | Tok::KeyWord(kw::COALESCE) => {
                Ok(ValueExpression::Case(Box::new(self.case_expression()?)))
            }
            Tok::KeyWord(kw::CAST) => self.cast_specification(),
            Tok::KeyWord(kw::CURRENT_DATE | kw::CURRENT_TIME | kw::CURRENT_TIMESTAMP) => {
                let f = self.datetime_value_function()?.ok_or_else(|| self.syntax("expected a <datetime value function>"))?;
                Ok(ValueExpression::Datetime(f))
            }
            Tok::KeyWord(kw::EXTRACT) => self.extract_expression(),
            Tok::KeyWord(kw::DATE | kw::TIME | kw::TIMESTAMP)
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
            | Tok::BitString(_) => Ok(ValueExpression::Value(self.value_specification()?)),
            // [ISO/IEC 9075:1992 §5.4] MODULE.T.C: a column of a declared
            // local temporary table.
            Tok::KeyWord(kw::MODULE) => Ok(ValueExpression::Column(self.column_specification()?)),
            Tok::KeyWord(kw::VALUE) => self.value_reference(),
            Tok::KeyWord(_) => self.string_function(),
            Tok::Delimiter(_) | Tok::End => Err(self.syntax("expected a <value expression>")),
        }
    }

    /// A `<primary>` that starts with an `<identifier>`: a function call, a
    /// `<parameter specification>`, or a `<column specification>`.
    fn identifier_primary(&mut self) -> Result<ValueExpression> {
        // An <identifier> followed by "(" never occurs in SQL-86.
        if matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            if !self.extensions {
                return Err(self.syntax(
                    "[X3.135-1986 §5.9 F] function calls are not SQL-86 (they are an opt-in [EXTENSION]); expected a <value expression>",
                ));
            }
            return self.function_call();
        }
        if self.at_parameter() {
            return Ok(ValueExpression::Value(ValueSpecification::Parameter(self.parameter_specification()?)));
        }
        Ok(ValueExpression::Column(self.column_specification()?))
    }

    /// [ISO/IEC 9075:1992 §6.5] `<set function specification>` ::= COUNT(*)
    /// | `<set function type>` (\[DISTINCT | ALL\] `<value expression>`)
    fn set_function_specification(&mut self) -> Result<SetFunctionSpecification> {
        let at = self.here();
        let Tok::KeyWord(name) = *self.peek() else {
            return Err(self.syntax("expected a <set function specification>"));
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
        Ok(SetFunctionSpecification { name, arg, at })
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
            let operand = self.value_expression()?;
            let mut whens = Vec::new();
            self.expect_kw(kw::WHEN)?;
            loop {
                let value = self.value_expression()?;
                self.expect_kw(kw::THEN)?;
                whens.push((value, self.result()?));
                if !self.accept_kw(kw::WHEN) {
                    break;
                }
            }
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

    /// [ISO/IEC 9075:1992 §6.10] `<cast specification>` ::= CAST (`<cast
    /// operand>` AS `<cast target>`), the operand a `<value expression>`
    /// or NULL, the target a `<data type>`.
    ///
    /// Not implemented: a `<domain name>` as the target.
    fn cast_specification(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_kw(kw::CAST)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let operand = if self.accept_kw(kw::NULL) { None } else { Some(Box::new(self.value_expression()?)) };
        self.expect_kw(kw::AS)?;
        let target = self.type_name()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Cast { operand, target, at })
    }

    /// [ISO/IEC 9075:1992 §6.6, §6.7] A string function, called with its
    /// operands in order (`functions/strings.rs`): SUBSTRING, UPPER,
    /// LOWER, TRIM, POSITION, CHAR_LENGTH or CHARACTER_LENGTH,
    /// OCTET_LENGTH, BIT_LENGTH.
    ///
    /// Not implemented: CONVERT and TRANSLATE, of character sets.
    fn string_function(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let Tok::KeyWord(word) = *self.peek() else { return Err(self.syntax("expected a <value expression>")) };
        let name = match word {
            kw::SUBSTRING | kw::UPPER | kw::LOWER | kw::TRIM | kw::POSITION | kw::OCTET_LENGTH | kw::BIT_LENGTH => word,
            kw::CHAR_LENGTH | kw::CHARACTER_LENGTH => kw::CHAR_LENGTH,
            kw::CONVERT | kw::TRANSLATE => {
                return Err(self.unsupported(&format!("[ISO/IEC 9075:1992 §6.6, §6.7] {word}")));
            }
            _ => return Err(self.syntax("expected a <value expression>")),
        };
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let args = match name {
            kw::SUBSTRING => self.substring_operands()?,
            kw::TRIM => self.trim_operands()?,
            kw::POSITION => {
                // POSITION (<character value expression> IN <character value expression>)
                let needle = self.value_expression()?;
                self.expect_kw(kw::IN)?;
                vec![needle, self.value_expression()?]
            }
            _ => vec![self.value_expression()?],
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Function { name, args, at })
    }

    /// SUBSTRING (`<character value expression>` FROM `<start position>`
    /// \[FOR `<string length>`\]): its operands.
    fn substring_operands(&mut self) -> Result<Vec<ValueExpression>> {
        let source = self.value_expression()?;
        self.expect_kw(kw::FROM)?;
        let mut args = vec![source, self.value_expression()?];
        if self.accept_kw(kw::FOR) {
            args.push(self.value_expression()?);
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
                let c = self.value_expression()?;
                self.expect_kw(kw::FROM)?;
                c
            };
            return Ok(vec![literal(w), character, self.value_expression()?]);
        }
        let first = self.value_expression()?;
        if self.accept_kw(kw::FROM) {
            // [SR6a] FROM after a <trim character>.
            return Ok(vec![literal(kw::BOTH), first, self.value_expression()?]);
        }
        Ok(vec![literal(kw::BOTH), literal(" "), first])
    }

    /// \[EXTENSION\] `<identifier>` ( \[`<value expression>` \[{, `<value expression>`}...\]\] )
    fn function_call(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let name = self.identifier()?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut args = Vec::new();
        if !self.accept_delim(delim::RIGHT_PAREN) {
            args.push(self.value_expression()?);
            while self.accept_delim(delim::COMMA) {
                args.push(self.value_expression()?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(ValueExpression::Call { name, args, at })
    }
}
