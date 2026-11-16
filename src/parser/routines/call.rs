//! [ISO/IEC 9075-2:2023 §16.1, §10.4, §16.2] The `<call statement>`,
//! with the `<SQL argument>`s turborel takes, and the `<return
//! statement>`, which is in a function's body alone.

use super::{CALL, RETURN, RoutineScope, not_supported};
use crate::ast::{CallStatement, ReturnStatement, RoutineKind, SqlArgument};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// COPARTITION, which begins a `<co-partition clause>`.
const COPARTITION: &str = "COPARTITION";

impl Parser {
    /// Whether a `<call statement>` begins here.
    pub(in crate::parser) fn at_call(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(w) if w == CALL) && matches!(self.peek_at(1), Tok::Identifier(_) | Tok::KeyWord(_))
    }

    /// [ISO/IEC 9075-2:2023 §16.1] `<call statement>` ::= CALL `<routine
    /// invocation>`; [§10.4] `<routine invocation>` ::= `<routine name>`
    /// `<SQL argument list>`.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(in crate::parser) fn call_statement(&mut self) -> Result<CallStatement> {
        self.expect_word(CALL)?;
        let name = self.table_name()?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut arguments = Vec::new();
        if self.accept_delim(delim::RIGHT_PAREN) {
            return Ok(CallStatement { name, arguments });
        }
        loop {
            arguments.push(self.sql_argument()?);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        if self.is_word(COPARTITION) {
            let msg = "[ISO/IEC 9075-2:2023 §10.4 CR10] a <co-partition clause> needs Feature B200, “Polymorphic table functions”, which \
                       turborel does not support";
            return Err(not_supported(self.here(), msg));
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(CallStatement { name, arguments })
    }

    /// [ISO/IEC 9075-2:2023 §10.4] `<SQL argument>`, of those implemented:
    /// a `<value expression>`, which a `<target specification>` is written
    /// as, DEFAULT or NULL.
    fn sql_argument(&mut self) -> Result<SqlArgument> {
        let at = self.here();
        let assignment = *self.peek_at(1) == Tok::Delimiter(delim::EQUALS) && *self.peek_at(2) == Tok::Delimiter(delim::GREATER);
        if matches!(self.peek(), Tok::Identifier(_)) && assignment {
            let msg = "[ISO/IEC 9075-2:2023 §10.4 CR5] a <named argument specification> in a CALL needs Feature T521, “Named arguments \
                       in CALL statement”, which turborel does not support";
            self.refuse_later(not_supported(at, msg));
            self.advance();
            self.advance();
            self.advance();
        }
        if self.is_kw(kw::TABLE) {
            let msg = "[ISO/IEC 9075-2:2023 §10.4 CR10] a <table argument> needs Feature B200, “Polymorphic table functions”, which \
                       turborel does not support";
            return Err(not_supported(at, msg));
        }
        if self.accept_kw(kw::DEFAULT) {
            return Ok(SqlArgument::Default(at));
        }
        if self.accept_kw(kw::NULL) {
            return Ok(SqlArgument::Null(at));
        }
        let value = self.value_expression()?;
        if self.is_kw(kw::AS) {
            let msg = "[ISO/IEC 9075-2:2023 §10.4 CR1] a <generalized expression> needs Feature S023, “Basic structured types”, which \
                       turborel does not support";
            return Err(not_supported(self.here(), msg));
        }
        Ok(SqlArgument::Value(value))
    }

    /// Whether a `<return statement>` begins here.
    pub(in crate::parser) fn at_return(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(w) if w == RETURN)
    }

    /// [ISO/IEC 9075-2:2023 §16.2] `<return statement>` ::= RETURN
    /// `<return value>`, `<return value>` ::= `<value expression>` | NULL.
    ///
    /// # Errors
    ///
    /// The text is not one, or it is not in a function's body \[SR1\].
    pub(in crate::parser) fn return_statement(&mut self) -> Result<ReturnStatement> {
        let at = self.here();
        let in_function = matches!(self.routine, Some(RoutineScope { kind: RoutineKind::Function, .. }));
        if !in_function {
            return Err(self.rule(
                "[ISO/IEC 9075-2:2023 §16.2 SR1] a <return statement> shall be contained in the <SQL routine body> of an SQL-invoked \
                 function",
            ));
        }
        self.expect_word(RETURN)?;
        let value = if self.accept_kw(kw::NULL) { None } else { Some(self.value_expression()?) };
        Ok(ReturnStatement { value, at })
    }
}
