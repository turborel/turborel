//! The `<data type>` grammar [X3.135-1986 §5.5], with SQL-92's national
//! character, datetime and interval types [ISO/IEC 9075:1992 §6.1],
//! SQL:2023's Boolean, binary string and decimal floating-point types
//! [ISO/IEC 9075-2:2023 §6.1],
//! the implementor-defined precisions and the \[DESIGN\] limits it
//! applies.

use super::Parser;
use crate::error::{Code, Result, SqlError};
use crate::functions::{CHARACTERS, OCTETS};
use crate::lexer::{Tok, delim, kw};
use crate::value::Spelled;
use crate::value::types::{BIGINT_NAME, BINARY_NAME, BOOLEAN_NAME, DECFLOAT_NAME, VARBINARY_NAME};
use crate::value::{
    BIGINT_PRECISION, CharUnits, DEFAULT_DECFLOAT_PRECISION, DEFAULT_EXACT_PRECISION, DOUBLE_PRECISION, DataType, INTEGER_PRECISION,
    MAX_BINARY_LENGTH, MAX_CHARACTER_LENGTH, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, REAL_PRECISION, SMALLINT_PRECISION,
};

impl Parser {
    /// [X3.135-1986 §5.5] `<predefined type>` [ISO/IEC 9075-2:2023 §6.1].
    pub(super) fn predefined_type(&mut self) -> Result<DataType> {
        if let Some(t) = self.named_type()? {
            return Ok(t);
        }
        let Tok::KeyWord(kw) = *self.peek() else {
            return Err(self.syntax("expected a <data type>"));
        };
        let at = self.here();
        match kw {
            kw::CHARACTER | kw::CHAR | kw::VARCHAR => {
                let t = self.character_string_type()?;
                // [ISO/IEC 9075:1992 §6.1] <character string type> [CHARACTER
                // SET <character set specification>]: SQL_TEXT's taken.
                self.character_set_clause()?;
                Ok(t)
            }
            kw::NCHAR => self.character_string_type(),
            // [ISO/IEC 9075:1992 §6.1 SR2] NATIONAL CHARACTER is CHARACTER of
            // the national character set, SQL_TEXT, the one there is.
            kw::NATIONAL => {
                self.advance();
                if !matches!(self.peek(), Tok::KeyWord(kw::CHARACTER | kw::CHAR)) {
                    return Err(self.syntax("expected CHARACTER or CHAR"));
                }
                self.character_string_type()
            }
            kw::DATE | kw::TIME | kw::TIMESTAMP => self.datetime_type(),
            // [ISO/IEC 9075:1992 §6.1] `<interval type>` ::= INTERVAL
            // `<interval qualifier>`.
            kw::INTERVAL => {
                self.advance();
                Ok(DataType::Interval(self.interval_qualifier()?))
            }
            kw::NUMERIC => self.exact_numeric_type(at, Spelled::Numeric),
            kw::DECIMAL | kw::DEC => self.exact_numeric_type(at, Spelled::Decimal),
            kw::INTEGER | kw::INT => {
                self.advance();
                Ok(DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer })
            }
            kw::SMALLINT => {
                self.advance();
                Ok(DataType::Exact { precision: SMALLINT_PRECISION, scale: 0, spelled: Spelled::Smallint })
            }
            kw::FLOAT => self.approximate_numeric_type(at),
            kw::REAL => {
                self.advance();
                Ok(DataType::Approximate { precision: REAL_PRECISION, spelled: Spelled::Real })
            }
            kw::DOUBLE => {
                self.advance();
                self.expect_kw(kw::PRECISION)?;
                Ok(DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision })
            }
            _ => Err(self.syntax("expected a <data type>")),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.1] A `<boolean type>`, a `<binary string
    /// type>`, a `<decimal floating-point type>` or BIGINT, if one is
    /// next, of names no key words of SQL-92.
    fn named_type(&mut self) -> Result<Option<DataType>> {
        if self.at_boolean_type() {
            self.advance();
            return Ok(Some(DataType::Boolean));
        }
        // [SR30] BIGINT, exact of scale 0 and of a precision no smaller
        // than INTEGER's.
        if self.at_bigint_type() {
            self.advance();
            return Ok(Some(DataType::Exact { precision: BIGINT_PRECISION, scale: 0, spelled: Spelled::Bigint }));
        }
        if self.at_binary_type() {
            return self.binary_string_type().map(Some);
        }
        if self.at_decfloat_type() {
            return self.decfloat_type().map(Some);
        }
        Ok(None)
    }

    /// [ISO/IEC 9075-2:2023 §6.1] Whether a `<decimal floating-point
    /// type>` is next, DECFLOAT, read as an identifier as BOOLEAN is.
    pub(super) fn at_decfloat_type(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(name) if name == DECFLOAT_NAME)
    }

    /// [ISO/IEC 9075-2:2023 §6.1] `<decimal floating-point type>` ::=
    /// DECFLOAT \[(`<precision>`)\]: of 34 digits if none is written
    /// (ID062), of 1 to 34 (IA021) [SR23].
    fn decfloat_type(&mut self) -> Result<DataType> {
        let at = self.here();
        self.advance();
        let mut precision = DEFAULT_DECFLOAT_PRECISION;
        if self.accept_delim(delim::LEFT_PAREN) {
            precision = self.positive("precision")?;
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        if precision > MAX_DECFLOAT_PRECISION {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!(
                    "[ISO/IEC 9075-2:2023 §6.1 SR23] DECFLOAT({precision}) exceeds this implementation's maximum of {MAX_DECFLOAT_PRECISION} digits"
                ),
            ));
        }
        Ok(DataType::DecFloat { precision })
    }

    /// [ISO/IEC 9075-2:2023 §6.1] Whether `<boolean type>` ::= BOOLEAN is
    /// next. BOOLEAN is no key word of SQL-92, so it is read as an
    /// identifier, and a domain cannot be named BOOLEAN.
    pub(super) fn at_boolean_type(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(name) if name == BOOLEAN_NAME)
    }

    /// [ISO/IEC 9075-2:2023 §6.1] Whether the `<exact numeric type>`
    /// BIGINT is next, read as an identifier as BOOLEAN is.
    pub(super) fn at_bigint_type(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(name) if name == BIGINT_NAME)
    }

    /// [ISO/IEC 9075-2:2023 §6.1] Whether a `<binary string type>` is
    /// next, BINARY or VARBINARY, read as identifiers as BOOLEAN is.
    pub(super) fn at_binary_type(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(name) if name == BINARY_NAME || name == VARBINARY_NAME)
    }

    /// `<character string type>` ::= CHARACTER \[(`<length>`)\] | CHAR \[(`<length>`)\]
    /// [ISO/IEC 9075:1992 §6.1] `<character string type>`: CHARACTER or
    /// CHAR \[(length)\]; CHARACTER VARYING, CHAR VARYING or VARCHAR
    /// \[(length)\], of this implementation's longest string if none is
    /// written [ISO/IEC 9075-2:2023 §6.1 SR7, Feature T081] (ID069). Of a
    /// `<national character string type>`, after NATIONAL, or NCHAR
    /// \[VARYING\], the same [SR1, SR2].
    fn character_string_type(&mut self) -> Result<DataType> {
        let varchar = *self.peek() == Tok::KeyWord(kw::VARCHAR);
        self.advance();
        if varchar || self.accept_kw(kw::VARYING) {
            let (length, units) = self.character_length(MAX_CHARACTER_LENGTH)?;
            return Ok(DataType::Varying { length, units });
        }
        // [X3.135-1986 §5.5 SR3] omitted <length> is 1
        let (length, units) = self.character_length(1)?;
        Ok(DataType::Character { length, units })
    }

    /// [ISO/IEC 9075-2:2023 §6.1] \[(`<character length>`)\], ::= `<length>`
    /// \[`<char length units>`\]: its length, `implicit` if none is written,
    /// and its units, CHARACTERS if none are [SR9; Feature T062].
    fn character_length(&mut self, implicit: u32) -> Result<(u32, CharUnits)> {
        if !self.accept_delim(delim::LEFT_PAREN) {
            return Ok((implicit, CharUnits::Characters));
        }
        let length = self.string_length()?;
        let units = if self.accept_word(OCTETS) {
            CharUnits::Octets
        } else {
            self.accept_word(CHARACTERS);
            CharUnits::Characters
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok((length, units))
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

    /// [ISO/IEC 9075-2:2023 §6.1] `<binary string type>` ::= BINARY
    /// \[(`<length>`)\] | BINARY VARYING \[(`<maximum length>`)\] |
    /// VARBINARY \[(`<maximum length>`)\], the same [SR1]; an omitted length
    /// 1 [SR5], an omitted maximum length this implementation's longest
    /// binary string [SR6, Feature T081] (IL007).
    fn binary_string_type(&mut self) -> Result<DataType> {
        let varbinary = matches!(self.peek(), Tok::Identifier(name) if name == VARBINARY_NAME);
        self.advance();
        if varbinary || self.accept_kw(kw::VARYING) {
            let mut length = MAX_BINARY_LENGTH;
            if self.accept_delim(delim::LEFT_PAREN) {
                length = self.binary_length()?;
                self.expect_delim(delim::RIGHT_PAREN)?;
            }
            return Ok(DataType::VarBinary { length });
        }
        let mut length = 1;
        if self.accept_delim(delim::LEFT_PAREN) {
            length = self.binary_length()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(DataType::Binary { length })
    }

    /// [ISO/IEC 9075-2:2023 §6.1 SR4] A binary string type's `<length>`:
    /// more than 0, and no more than this implementation's longest binary
    /// string, in octets.
    fn binary_length(&mut self) -> Result<u32> {
        let at = self.here();
        let length = self.positive("length")?;
        if length > MAX_BINARY_LENGTH {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!("[DESIGN] a binary string of {length} octets is longer than this implementation's maximum of {MAX_BINARY_LENGTH}"),
            ));
        }
        Ok(length)
    }

    /// NUMERIC | DECIMAL | DEC \[(`<precision>` \[,`<scale>`\])\], starting at `at`,
    /// as `spelled` says it was written.
    fn exact_numeric_type(&mut self, at: usize, spelled: Spelled) -> Result<DataType> {
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
        Ok(DataType::Exact { precision, scale, spelled })
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
        Ok(DataType::Approximate {
            precision: if precision <= REAL_PRECISION { REAL_PRECISION } else { DOUBLE_PRECISION },
            spelled: Spelled::Float,
        })
    }
}
