//! [ISO/IEC 9075:1992 §6.6, §6.7; ISO/IEC 9075-2:2023 §6.31, §6.33] The
//! string functions read with their own syntax, each called with its
//! operands in order (`functions/strings.rs`, `functions/trim.rs`):
//! SUBSTRING, UPPER, LOWER, TRIM, POSITION, the lengths, and OVERLAY.

use crate::ast::{Literal, ValueExpression, ValueSpecification};
use crate::error::Result;
use crate::functions::{OVERLAY, PLACING, using};
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// A character string literal of `s`, an operand the syntax implies.
fn literal(s: &str) -> ValueExpression {
    ValueExpression::Value(ValueSpecification::Literal(Literal::Character(s.to_owned())))
}

impl Parser {
    /// [ISO/IEC 9075:1992 §6.6, §6.7] A string function, called with its
    /// operands in order (`functions/strings.rs`): SUBSTRING, UPPER,
    /// LOWER, TRIM, POSITION, CHAR_LENGTH or CHARACTER_LENGTH,
    /// OCTET_LENGTH, BIT_LENGTH.
    ///
    /// Read and refused: CONVERT and TRANSLATE, of character sets
    /// (`Parser::conversion`).
    pub(super) fn string_function(&mut self) -> Result<ValueExpression> {
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
    /// `<trim source>`), or of a binary string [ISO/IEC 9075-2:2023 §6.33]
    /// `<binary trim operands>`: the source; the specification, BOTH if
    /// none \[SR14a i 2, SR22a i 2\], as a string; and the character or
    /// octet if written, which the trim takes as a space or X'00' if not,
    /// by its source's type \[SR14a i 3, SR22a i 3\].
    fn trim_operands(&mut self) -> Result<Vec<ValueExpression>> {
        let spec = [kw::LEADING, kw::TRAILING, kw::BOTH].into_iter().find(|w| self.is_kw(w));
        if let Some(w) = spec {
            self.advance();
            if self.accept_kw(kw::FROM) {
                return Ok(vec![self.common_value_expression()?, literal(w)]);
            }
            let c = self.common_value_expression()?;
            self.expect_kw(kw::FROM)?;
            return Ok(vec![self.common_value_expression()?, literal(w), c]);
        }
        // [ISO/IEC 9075:1992 §6.7 SR6b, SR6c] FROM with neither.
        if self.accept_kw(kw::FROM) {
            return Ok(vec![self.common_value_expression()?, literal(kw::BOTH)]);
        }
        let first = self.common_value_expression()?;
        if self.accept_kw(kw::FROM) {
            // [SR14a i] FROM after a <trim character>.
            return Ok(vec![self.common_value_expression()?, literal(kw::BOTH), first]);
        }
        // [SR14a ii, SR22a ii] TRIM (SRC) is TRIM (BOTH FROM SRC).
        Ok(vec![first, literal(kw::BOTH)])
    }

    /// [ISO/IEC 9075-2:2023 §6.33] `<character overlay function>` ::=
    /// OVERLAY ( `<character value expression>` PLACING `<character value
    /// expression>` FROM `<start position>` \[FOR `<string length>`\]
    /// \[USING `<char length units>`\] ), and `<binary overlay function>`,
    /// of binary value expressions and no USING: called with the string,
    /// its replacement, the start and the length if written.
    pub(super) fn overlay(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut args = vec![self.common_value_expression()?];
        if !self.accept_word(PLACING) {
            return Err(self.syntax("expected PLACING in OVERLAY"));
        }
        args.push(self.common_value_expression()?);
        self.expect_kw(kw::FROM)?;
        args.push(self.common_value_expression()?);
        if self.accept_kw(kw::FOR) {
            args.push(self.common_value_expression()?);
        }
        let name = match self.char_length_units()? {
            Some(units) => using(OVERLAY, units).ok_or_else(|| self.syntax("USING follows no OVERLAY"))?,
            None => OVERLAY,
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Function { name, args, at })
    }
}
