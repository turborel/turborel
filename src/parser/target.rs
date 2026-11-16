//! [X3.135-1986 §8.6, §8.10; ISO/IEC 9075-2:2023 §6.4] The targets of
//! FETCH and SELECT INTO: a host parameter of a module's procedure, an
//! SQL parameter of a procedure's body, a new transition variable's column
//! of a BEFORE trigger's action, or an element of the array either of the
//! last two holds, a `<target array element specification>` (Feature
//! S097).

use super::Parser;
use crate::ast::{ElementPosition, ParameterSpecification, TargetKind};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim};

impl Parser {
    /// [X3.135-1986 §8.6, §8.10] `<fetch target list>` and `<select target list>`
    /// ::= `<target specification>` \[{, `<target specification>`}...\]
    pub(super) fn target_list(&mut self) -> Result<Vec<ParameterSpecification>> {
        let mut list = vec![self.target_specification()?];
        while self.accept_delim(delim::COMMA) {
            list.push(self.target_specification()?);
        }
        Ok(list)
    }

    /// [X3.135-1986 §5.6] `<target specification>` ::= `<parameter
    /// specification>` (not implemented: `<variable specification>`, which
    /// is embedded SQL); [ISO/IEC 9075-2:2023 §6.4] in a procedure's body
    /// or a triggered action, `<SQL parameter reference>`, `<column
    /// reference>`, or `<target array element specification>` ::= `<target
    /// array reference>` `[` `<simple value specification>` `]`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if an element is of a host parameter, which no `<target
    /// array reference>` is.
    pub(super) fn target_specification(&mut self) -> Result<ParameterSpecification> {
        let mut spec = if self.trigger.is_some() || self.routine.is_some() {
            self.scoped_target()?
        } else if self.is_delim(delim::COLON) {
            return Err(self.unsupported("<variable specification> (embedded SQL)"));
        } else if !matches!(self.peek(), Tok::Identifier(_)) {
            return Err(self.syntax("expected a <target specification>"));
        } else {
            self.parameter_specification()?
        };
        if !self.is_delim(delim::LEFT_BRACKET) {
            return Ok(spec);
        }
        if spec.kind == TargetKind::Parameter {
            return Err(self.rule(
                "[ISO/IEC 9075-2:2023 §6.4] the array of a <target array element specification> is an <SQL parameter reference> or a \
                 new transition variable column reference, not a host parameter",
            ));
        }
        self.advance();
        spec.element = Some(Box::new(self.element_position()?));
        self.expect_delim(delim::RIGHT_BRACKET)?;
        Ok(spec)
    }

    /// [ISO/IEC 9075-2:2023 §6.4] The `<simple value specification>` of a
    /// `<target array element specification>`: an `<SQL parameter
    /// reference>` of the procedure whose body it is in, or a `<literal>`.
    ///
    /// A host or dynamic parameter, which neither a procedure's body nor a
    /// triggered action holds, is refused as they are read [§11.60 SR19,
    /// §11.49 SR10].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is a name that is no SQL parameter.
    fn element_position(&mut self) -> Result<ElementPosition> {
        if let Some((name, qualified)) = self.parameter_reference() {
            self.advance();
            if qualified {
                self.advance();
                self.advance();
            }
            return Ok(ElementPosition::Parameter(name));
        }
        if matches!(self.peek(), Tok::Identifier(_)) && !self.at_datetime_literal() && !self.at_interval_literal() {
            let may = if self.routine.is_some() { "a <literal> or an <SQL parameter reference>" } else { "a <literal>" };
            return Err(SqlError::at(
                Code::SyntaxRule,
                self.here(),
                format!(
                    "[ISO/IEC 9075-2:2023 §6.4] the position of the element a target assigns is a <simple value specification>, here {may}"
                ),
            ));
        }
        Ok(ElementPosition::Literal(self.literal()?))
    }
}
