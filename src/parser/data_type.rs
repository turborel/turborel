//! The `<data type>` grammar [X3.135-1986 §5.5], with the implementor-defined
//! precisions and the \[DESIGN\] limits it applies. Split from parser.rs
//! (RULES.md R-SIZE-2).

use super::Parser;
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::value::{
    DEFAULT_EXACT_PRECISION, DOUBLE_PRECISION, DataType, INTEGER_PRECISION, MAX_CHARACTER_LENGTH, MAX_EXACT_PRECISION, REAL_PRECISION,
    SMALLINT_PRECISION,
};

impl Parser {
    /// [X3.135-1986 §5.5] `<data type>`
    pub(super) fn data_type(&mut self) -> Result<DataType> {
        let Tok::KeyWord(kw) = *self.peek() else {
            return Err(self.syntax("expected a <data type>"));
        };
        let at = self.here();
        match kw {
            kw::CHARACTER | kw::CHAR => self.character_string_type(),
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
    fn character_string_type(&mut self) -> Result<DataType> {
        self.advance();
        let mut length = 1; // [X3.135-1986 §5.5 SR3] omitted <length> is 1
        if self.accept_delim(delim::LEFT_PAREN) {
            let length_at = self.here();
            length = self.positive("length")?;
            if length > MAX_CHARACTER_LENGTH {
                return Err(SqlError::at(
                    Code::Unsupported,
                    length_at,
                    format!("[DESIGN] CHARACTER({length}) is longer than this implementation's maximum of {MAX_CHARACTER_LENGTH}"),
                ));
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(DataType::Character { length })
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
