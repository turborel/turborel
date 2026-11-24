//! [ISO/IEC 9075-2:2023 §16.1, §10.4, §16.2] The `<call statement>`,
//! with the `<SQL argument>`s turborel takes, and the `<return
//! statement>`, which is in a function's body alone.

use super::{CALL, COPARTITION, RETURN, RoutineScope, not_supported};
use crate::ast::{CallStatement, ReturnStatement, RoutineKind, SqlArgument};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// [ISO/IEC 9075-2:2023 §9.18 SR9a] What no CALL's invocation holds.
const SR9A: &str = "[ISO/IEC 9075-2:2023 §9.18 SR9a] the <routine invocation> of a CALL holds no <table argument>, <descriptor argument> or \
     <co-partition clause>";

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
        let (mut arguments, mut names, mut dynamic) = (Vec::new(), Vec::new(), Vec::new());
        if self.accept_delim(delim::RIGHT_PAREN) {
            return Ok(CallStatement { name, arguments, names, dynamic });
        }
        loop {
            names.push(self.named_argument(&names)?);
            let before = self.questions_before();
            arguments.push(self.sql_argument()?);
            dynamic.push(self.questions_since(before));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        if self.is_word(COPARTITION) {
            return Err(self.rule(SR9A));
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(CallStatement { name, arguments, names, dynamic })
    }

    /// [ISO/IEC 9075-2:2023 §10.4] `<SQL argument>`, of those implemented:
    /// a `<value expression>`, which a `<target specification>` is written
    /// as, DEFAULT or NULL.
    fn sql_argument(&mut self) -> Result<SqlArgument> {
        let at = self.here();
        if self.is_kw(kw::TABLE) || self.is_kw(kw::DESCRIPTOR) || (self.is_kw(kw::CAST) && self.next_is_null_descriptor()) {
            return Err(self.rule(SR9A));
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
