//! [ISO/IEC 9075:1992 §5.3, §6.1, §6.6, §6.8] The datetime grammar: the
//! `<datetime type>`s, the `<datetime literal>`s, the `<datetime value
//! function>`s and the `<extract expression>`. Split from parser.rs
//! (RULES.md R-SIZE-2); the methods extend the same `Parser`.

use super::Parser;
use crate::ast::{DatetimeFunction, ExtractField, Literal, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::value::DataType;
use crate::value::datetime::{Field, MAX_TIME_PRECISION, Moment, parse};

/// The datetime type, literal or value function key word `word` names,
/// and which.
fn moment_of(word: &str) -> Option<Moment> {
    match word {
        kw::DATE | kw::CURRENT_DATE => Some(Moment::Date),
        kw::TIME | kw::CURRENT_TIME => Some(Moment::Time),
        kw::TIMESTAMP | kw::CURRENT_TIMESTAMP => Some(Moment::Timestamp),
        _ => None,
    }
}

impl Parser {
    /// Whether a `<datetime literal>` is next: DATE, TIME or TIMESTAMP, then
    /// a character string.
    pub(super) fn at_datetime_literal(&self) -> bool {
        matches!(self.peek(), Tok::KeyWord(kw::DATE | kw::TIME | kw::TIMESTAMP)) && matches!(self.peek_at(1), Tok::CharacterString(_))
    }

    /// [ISO/IEC 9075:1992 §5.3] `<datetime literal>` ::= DATE `<date string>`
    /// | TIME `<time string>` | TIMESTAMP `<timestamp string>`, its string of
    /// the fields and digits SR21 says, valid in the calendar [SR22], its
    /// displacement from -12:59 to +13:00 [SR25].
    pub(super) fn datetime_literal(&mut self) -> Result<Literal> {
        let at = self.here();
        let Tok::KeyWord(word) = *self.peek() else { return Err(self.syntax("expected a <datetime literal>")) };
        let moment = moment_of(word).ok_or_else(|| self.syntax("expected a <datetime literal>"))?;
        self.advance();
        let Tok::CharacterString(text) = self.peek().clone() else { return Err(self.syntax("expected a <datetime literal>'s string")) };
        let Some(written) = parse(moment, &text) else {
            let msg = format!("[ISO/IEC 9075:1992 §5.3 SR21, SR22, SR25] {word} '{text}' is no valid <datetime literal>");
            return Err(SqlError::at(Code::SyntaxRule, at, msg));
        };
        if written.precision > MAX_TIME_PRECISION {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!("[DESIGN] {word} '{text}' has more digits of the second than {MAX_TIME_PRECISION}"),
            ));
        }
        self.advance();
        Ok(Literal::Datetime(written))
    }

    /// [ISO/IEC 9075:1992 §6.8] `<datetime value function>` ::=
    /// CURRENT_DATE | CURRENT_TIME \[(`<time precision>`)\] |
    /// CURRENT_TIMESTAMP \[(`<timestamp precision>`)\], taken if one is
    /// next.
    pub(super) fn datetime_value_function(&mut self) -> Result<Option<DatetimeFunction>> {
        let Tok::KeyWord(word @ (kw::CURRENT_DATE | kw::CURRENT_TIME | kw::CURRENT_TIMESTAMP)) = *self.peek() else { return Ok(None) };
        let moment = moment_of(word).ok_or_else(|| self.syntax("expected a <datetime value function>"))?;
        self.advance();
        let precision = if moment != Moment::Date && self.accept_delim(delim::LEFT_PAREN) {
            let p = self.time_precision()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            Some(p)
        } else {
            None
        };
        Ok(Some(DatetimeFunction { moment, precision }))
    }

    /// [ISO/IEC 9075:1992 §6.6] `<extract expression>` ::= EXTRACT
    /// (`<extract field>` FROM `<extract source>`), `<extract field>` ::=
    /// `<datetime field>` | TIMEZONE_HOUR | TIMEZONE_MINUTE.
    pub(super) fn extract_expression(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_kw(kw::EXTRACT)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let field = match *self.peek() {
            Tok::KeyWord(kw::YEAR) => ExtractField::Datetime(Field::Year),
            Tok::KeyWord(kw::MONTH) => ExtractField::Datetime(Field::Month),
            Tok::KeyWord(kw::DAY) => ExtractField::Datetime(Field::Day),
            Tok::KeyWord(kw::HOUR) => ExtractField::Datetime(Field::Hour),
            Tok::KeyWord(kw::MINUTE) => ExtractField::Datetime(Field::Minute),
            Tok::KeyWord(kw::SECOND) => ExtractField::Datetime(Field::Second),
            Tok::KeyWord(kw::TIMEZONE_HOUR) => ExtractField::TimezoneHour,
            Tok::KeyWord(kw::TIMEZONE_MINUTE) => ExtractField::TimezoneMinute,
            Tok::KeyWord(_)
            | Tok::Identifier(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Delimiter(_)
            | Tok::End => return Err(self.syntax("expected an <extract field>")),
        };
        self.advance();
        self.expect_kw(kw::FROM)?;
        let source = Box::new(self.value_expression()?);
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Extract { field, source, at })
    }

    /// [ISO/IEC 9075:1992 §6.1] `<datetime type>` ::= DATE | TIME
    /// \[(`<time precision>`)\] \[WITH TIME ZONE\] | TIMESTAMP
    /// \[(`<timestamp precision>`)\] \[WITH TIME ZONE\]; of an omitted
    /// precision 0 for TIME and 6 for TIMESTAMP [SR25].
    pub(super) fn datetime_type(&mut self) -> Result<DataType> {
        let Tok::KeyWord(word) = *self.peek() else { return Err(self.syntax("expected a <datetime type>")) };
        let moment = moment_of(word).ok_or_else(|| self.syntax("expected a <datetime type>"))?;
        self.advance();
        if moment == Moment::Date {
            return Ok(DataType::Date);
        }
        let mut precision = if moment == Moment::Time { 0 } else { 6 };
        if self.accept_delim(delim::LEFT_PAREN) {
            precision = self.time_precision()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        let zoned = self.accept_kw(kw::WITH);
        if zoned {
            self.expect_kw(kw::TIME)?;
            self.expect_kw(kw::ZONE)?;
        }
        Ok(if moment == Moment::Time { DataType::Time { precision, zoned } } else { DataType::Timestamp { precision, zoned } })
    }

    /// [ISO/IEC 9075:1992 §6.1 SR26] A `<time fractional seconds
    /// precision>`: no more than this implementation's, 6.
    fn time_precision(&mut self) -> Result<u32> {
        let at = self.here();
        let precision = self.unsigned_integer()?;
        if precision > MAX_TIME_PRECISION {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!(
                    "[DESIGN] a <time fractional seconds precision> of {precision} is more than this implementation's maximum of {MAX_TIME_PRECISION}"
                ),
            ));
        }
        Ok(precision)
    }
}
