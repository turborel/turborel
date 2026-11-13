//! [ISO/IEC 9075-2:2023 §10.9] The `<listagg set function>`, and the
//! `<sort specification list>` of it and of ARRAY_AGG. LISTAGG and WITHIN,
//! which SQL:2023 reserves and SQL-92 does not, and OVERFLOW, ERROR,
//! TRUNCATE and WITHOUT, which SQL-92 does not reserve either, are key
//! words only here: LISTAGG before a parenthesis, the others where the
//! function's syntax has them, so a column may have any of those names.

use super::Parser;
use super::data_statements::TRUNCATE;
use super::dynamic::WITHOUT;
use crate::ast::{Listagg, Literal, Overflow, SetArg, SetFunctionSpecification, ValueOrder};
use crate::error::Result;
use crate::functions::LISTAGG;
use crate::lexer::{Tok, delim, kw};

/// The words of a `<listagg set function>` that SQL-92 does not reserve.
const OVERFLOW: &str = "OVERFLOW";
const ERROR: &str = "ERROR";
const WITHIN: &str = "WITHIN";

/// \[SR11g\] The `<listagg truncation filler>` when none is written.
const DEFAULT_FILLER: &str = "...";

impl Parser {
    /// [ISO/IEC 9075-2:2023 §10.9] `<listagg set function>` ::= LISTAGG (
    /// \[`<set quantifier>`\] `<character value expression>` , `<listagg
    /// separator>` \[`<listagg overflow clause>`\] ) `<within group
    /// specification>`, LISTAGG next.
    pub(super) fn listagg(&mut self) -> Result<SetFunctionSpecification> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let distinct = self.accept_kw(kw::DISTINCT);
        if !distinct {
            // [SR11c] ALL, written or implicit.
            self.accept_kw(kw::ALL);
        }
        let value = Box::new(self.nested(Parser::value_expression)?);
        self.expect_delim(delim::COMMA)?;
        let separator = self.character_string_literal("<listagg separator>")?;
        let overflow = self.overflow_clause()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        // <within group specification> ::= WITHIN GROUP ( ORDER BY <sort
        // specification list> )
        if !self.accept_word(WITHIN) {
            return Err(self.syntax("expected WITHIN GROUP after LISTAGG's arguments [ISO/IEC 9075-2:2023 §10.9]"));
        }
        self.expect_kw(kw::GROUP)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        self.expect_kw(kw::ORDER)?;
        self.expect_kw(kw::BY)?;
        let order_by = self.sort_specification_list()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let arg = if distinct { SetArg::Distinct(value) } else { SetArg::All(value) };
        let listagg = Some(Box::new(Listagg { separator, overflow }));
        Ok(SetFunctionSpecification { name: LISTAGG, arg, order_by, listagg, at })
    }

    /// `<listagg overflow clause>` ::= ON OVERFLOW `<overflow behavior>`, if
    /// written; else ERROR \[SR11f\].
    fn overflow_clause(&mut self) -> Result<Overflow> {
        if !self.accept_kw(kw::ON) {
            return Ok(Overflow::Error);
        }
        if !self.accept_word(OVERFLOW) {
            return Err(self.syntax("expected OVERFLOW after ON [ISO/IEC 9075-2:2023 §10.9]"));
        }
        if self.accept_word(ERROR) {
            return Ok(Overflow::Error);
        }
        if !self.accept_word(TRUNCATE) {
            return Err(self.syntax("expected ERROR or TRUNCATE after ON OVERFLOW [ISO/IEC 9075-2:2023 §10.9]"));
        }
        let written = matches!(self.peek(), Tok::CharacterString(_) | Tok::Introducer(_));
        let filler = if written { self.character_string_literal("<listagg truncation filler>")? } else { DEFAULT_FILLER.to_owned() };
        // <listagg count indication> ::= WITH COUNT | WITHOUT COUNT
        let count = self.accept_kw(kw::WITH);
        if !count && !self.accept_word(WITHOUT) {
            return Err(self.syntax("expected WITH COUNT or WITHOUT COUNT [ISO/IEC 9075-2:2023 §10.9]"));
        }
        self.expect_kw(kw::COUNT)?;
        Ok(Overflow::Truncate { filler, count })
    }

    /// A `<character string literal>`'s value, the `what` of the syntax.
    fn character_string_literal(&mut self, what: &str) -> Result<String> {
        let introduced = matches!(self.peek(), Tok::Introducer(_)) && matches!(self.peek_at(1), Tok::CharacterString(_));
        if !introduced && !matches!(self.peek(), Tok::CharacterString(_)) {
            return Err(self.syntax(format!("expected a <character string literal> as the {what} [ISO/IEC 9075-2:2023 §10.9]")));
        }
        match self.literal()? {
            Literal::Character(s) => Ok(s),
            Literal::Binary(_)
            | Literal::Exact(_)
            | Literal::Approximate(_)
            | Literal::Boolean(_)
            | Literal::Datetime(_)
            | Literal::Interval(_) => Err(self.syntax(format!("expected a <character string literal> as the {what}"))),
        }
    }

    /// [ISO/IEC 9075-2:2023 §10.10] `<sort specification list>`, each
    /// `<sort key>` a `<value expression>` \[ASC | DESC\] \[`<null
    /// ordering>`\]: of ARRAY_AGG and
    /// of a `<within group specification>`.
    pub(super) fn sort_specification_list(&mut self) -> Result<Vec<ValueOrder>> {
        let mut order_by = Vec::new();
        loop {
            let key = self.nested(Parser::value_expression)?;
            let descending = self.accept_kw(kw::DESC);
            if !descending {
                self.accept_kw(kw::ASC);
            }
            let nulls_first = self.null_ordering()?;
            order_by.push(ValueOrder { key, descending, nulls_first });
            if !self.accept_delim(delim::COMMA) {
                return Ok(order_by);
            }
        }
    }
}
