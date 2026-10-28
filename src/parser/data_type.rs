//! The `<data type>` grammar [X3.135-1986 §5.5], with the implementor-defined
//! precisions and the \[DESIGN\] limits it applies. Split from parser.rs
//! (RULES.md R-SIZE-2).

use super::Parser;
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::value::{
    DEFAULT_EXACT_PRECISION, DOUBLE_PRECISION, DataType, INTEGER_PRECISION, MAX_BIT_LENGTH, MAX_CHARACTER_LENGTH, MAX_EXACT_PRECISION,
    REAL_PRECISION, SMALLINT_PRECISION,
};

impl Parser {
    /// [X3.135-1986 §5.5] `<data type>`
    pub(super) fn data_type(&mut self) -> Result<DataType> {
        let Tok::KeyWord(kw) = *self.peek() else {
            return Err(self.syntax("expected a <data type>"));
        };
        let at = self.here();
        match kw {
            kw::CHARACTER | kw::CHAR | kw::VARCHAR | kw::NCHAR => self.character_string_type(),
            // [ISO/IEC 9075:1992 §6.1 SR2] NATIONAL CHARACTER is CHARACTER of
            // the national character set, SQL_TEXT, the one there is.
            kw::NATIONAL => {
                self.advance();
                if !matches!(self.peek(), Tok::KeyWord(kw::CHARACTER | kw::CHAR)) {
                    return Err(self.syntax("expected CHARACTER or CHAR"));
                }
                self.character_string_type()
            }
            kw::BIT => self.bit_string_type(),
            kw::NUMERIC | kw::DECIMAL | kw::DEC => self.exact_numeric_type(at),
            kw::INTEGER | kw::INT => {
                self.advance();
                Ok(DataType::Exact { precision: INTEGER_PRECISION, scale: 0 })
            }
            kw::SMALLINT => {
                self.advance();
                Ok(DataType::Exact { precision: SMALLINT_PRECISION, scale: 0 })
            }
            kw::FLOAT => self.approximate_numeric_type(at),
            kw::REAL => {
                self.advance();
                Ok(DataType::Approximate { precision: REAL_PRECISION })
            }
            kw::DOUBLE => {
                self.advance();
                self.expect_kw(kw::PRECISION)?;
                Ok(DataType::Approximate { precision: DOUBLE_PRECISION })
            }
            _ => Err(self.syntax("expected a <data type>")),
        }
    }

    /// `<character string type>` ::= CHARACTER \[(`<length>`)\] | CHAR \[(`<length>`)\]
    /// [ISO/IEC 9075:1992 §6.1] `<character string type>`: CHARACTER or
    /// CHAR \[(length)\]; CHARACTER VARYING, CHAR VARYING or VARCHAR
    /// (length), its length written. Of a `<national character string
    /// type>`, after NATIONAL, or NCHAR \[VARYING\], the same [SR1, SR2].
    fn character_string_type(&mut self) -> Result<DataType> {
        let varchar = *self.peek() == Tok::KeyWord(kw::VARCHAR);
        self.advance();
        if varchar || self.accept_kw(kw::VARYING) {
            self.expect_delim(delim::LEFT_PAREN)?;
            let length = self.string_length()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(DataType::Varying { length });
        }
        let mut length = 1; // [X3.135-1986 §5.5 SR3] omitted <length> is 1
        if self.accept_delim(delim::LEFT_PAREN) {
            length = self.string_length()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(DataType::Character { length })
    }

    /// [ISO/IEC 9075:1992 §6.1] A character string type's `<length>`: more
    /// than 0, and no more than this implementation's longest string, of
    /// fixed or variable length.
    fn string_length(&mut self) -> Result<u32> {
        let at = self.here();
        let length = self.positive("length")?;
        if length > MAX_CHARACTER_LENGTH {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!(
                    "[DESIGN] a character string of {length} characters is longer than this implementation's maximum of {MAX_CHARACTER_LENGTH}"
                ),
            ));
        }
        Ok(length)
    }

    /// [ISO/IEC 9075:1992 §6.1] `<bit string type>` ::= BIT \[(`<length>`)\]
    /// | BIT VARYING (`<length>`); an omitted length 1 [SR4].
    fn bit_string_type(&mut self) -> Result<DataType> {
        self.advance();
        if self.accept_kw(kw::VARYING) {
            self.expect_delim(delim::LEFT_PAREN)?;
            let length = self.bit_length()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(DataType::BitVarying { length });
        }
        let mut length = 1;
        if self.accept_delim(delim::LEFT_PAREN) {
            length = self.bit_length()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(DataType::Bit { length })
    }

    /// [ISO/IEC 9075:1992 §6.1 SR3] A bit string type's `<length>`: more
    /// than 0, and no more than this implementation's longest bit string.
    fn bit_length(&mut self) -> Result<u32> {
        let at = self.here();
        let length = self.positive("length")?;
        if length > MAX_BIT_LENGTH {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!("[DESIGN] a bit string of {length} bits is longer than this implementation's maximum of {MAX_BIT_LENGTH}"),
            ));
        }
        Ok(length)
    }

    /// NUMERIC | DECIMAL | DEC \[(`<precision>` \[,`<scale>`\])\], starting at `at`.
    fn exact_numeric_type(&mut self, at: usize) -> Result<DataType> {
        self.advance();
        let (mut precision, mut scale) = (DEFAULT_EXACT_PRECISION, 0); // [X3.135-1986 §5.5 SR3]
        if self.accept_delim(delim::LEFT_PAREN) {
            precision = self.positive("precision")?;
            if self.accept_delim(delim::COMMA) {
                scale = self.unsigned_integer()?;
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        if scale > precision {
            return Err(SqlError::at(Code::SyntaxRule, at, "[X3.135-1986 §5.5 SR4] <scale> shall not be greater than <precision>"));
        }
        // [X3.135-1986 §5.5 SR6, SR7] set no maximum: a larger precision is valid
        // SQL-86 that this implementation does not process (RULES.md D18).
        if precision > MAX_EXACT_PRECISION {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!(
                    "[X3.135-1986 §5.5 SR6, SR7] precision {precision} exceeds this implementation's maximum exact precision of {MAX_EXACT_PRECISION}"
                ),
            ));
        }
        Ok(DataType::Exact { precision, scale })
    }

    /// FLOAT \[(`<precision>`)\], starting at `at`.
    fn approximate_numeric_type(&mut self, at: usize) -> Result<DataType> {
        self.advance();
        let mut precision = DOUBLE_PRECISION; // [X3.135-1986 §5.5 SR3] implementor-defined when omitted
        if self.accept_delim(delim::LEFT_PAREN) {
            precision = self.positive("precision")?;
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        // [X3.135-1986 §5.5 SR10] sets no maximum: a larger precision is valid
        // SQL-86 that this implementation does not process (RULES.md D18).
        if precision > DOUBLE_PRECISION {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!(
                    "[X3.135-1986 §5.5 SR10] FLOAT({precision}) exceeds this implementation's maximum of {DOUBLE_PRECISION} binary digits"
                ),
            ));
        }
        // [X3.135-1986 §5.5 SR10] binary precision equal to or greater than requested.
        Ok(DataType::Approximate { precision: if precision <= REAL_PRECISION { REAL_PRECISION } else { DOUBLE_PRECISION } })
    }
}
