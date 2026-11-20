//! [ISO/IEC 9075:1992 §5.3, §6.1, §6.6, §6.8] The datetime grammar: the
//! `<datetime type>`s, the `<datetime literal>`s, the `<datetime value
//! function>`s, LOCALTIME and LOCALTIMESTAMP of ISO/IEC 9075-2:2023 §6.43
//! too, and the `<extract expression>`.

use super::Parser;
use crate::ast::{DatetimeFunction, ExtractField, Literal, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::value::DataType;
use crate::value::datetime::{Field, MAX_TIME_PRECISION, Moment, parse};

/// [ISO/IEC 9075-2:2023 §6.43] LOCALTIME, a `<reserved word>` there:
/// \[DESIGN\] here a `<regular identifier>` no period follows, so the
/// columns and names that spell it keep their meaning as CURRENT_ROLE's do
/// (RULES.md D75, D330).
pub(crate) const LOCALTIME: &str = "LOCALTIME";
/// [ISO/IEC 9075-2:2023 §6.43] LOCALTIMESTAMP, read as LOCALTIME is.
pub(crate) const LOCALTIMESTAMP: &str = "LOCALTIMESTAMP";

/// [ISO/IEC 9075-2:2023 §6.1] WITHOUT, of `<with or without time zone>`,
/// no key word of SQL-92: a word only after a time or timestamp type.
const WITHOUT: &str = "WITHOUT";

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
    /// displacement a valid one, from -12:59 to +14:00 [ISO/IEC 9075-2:2023
    /// §5.3 SR43].
    pub(super) fn datetime_literal(&mut self) -> Result<Literal> {
        let at = self.here();
        let Tok::KeyWord(word) = *self.peek() else { return Err(self.syntax("expected a <datetime literal>")) };
        let moment = moment_of(word).ok_or_else(|| self.syntax("expected a <datetime literal>"))?;
        self.advance();
        let Tok::CharacterString(text) = self.peek().clone() else { return Err(self.syntax("expected a <datetime literal>'s string")) };
        let Some(written) = parse(moment, &text) else {
            let msg = format!(
                "[ISO/IEC 9075:1992 §5.3 SR21, SR22; ISO/IEC 9075-2:2023 §5.3 SR43] {word} '{text}' is no valid <datetime literal>"
            );
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

    /// Whether LOCALTIME or LOCALTIMESTAMP is next, as
    /// `datetime_value_function` reads it: its token as wide as the word,
    /// so not delimited by double quotes, and no period after it; and
    /// which.
    pub(super) fn at_local_datetime(&self) -> Option<Moment> {
        let t = self.token_at(0);
        let Tok::Identifier(word) = &t.tok else { return None };
        let moment = if word == LOCALTIME {
            Moment::Time
        } else if word == LOCALTIMESTAMP {
            Moment::Timestamp
        } else {
            return None;
        };
        let regular = t.end.checked_sub(t.start) == Some(word.len());
        (regular && !matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD))).then_some(moment)
    }

    /// [ISO/IEC 9075:1992 §6.8] `<datetime value function>` ::=
    /// CURRENT_DATE | CURRENT_TIME \[(`<time precision>`)\] |
    /// CURRENT_TIMESTAMP \[(`<timestamp precision>`)\]; [ISO/IEC
    /// 9075-2:2023 §6.43] | LOCALTIME \[(`<time precision>`)\] |
    /// LOCALTIMESTAMP \[(`<timestamp precision>`)\]; taken if one is next.
    pub(super) fn datetime_value_function(&mut self) -> Result<Option<DatetimeFunction>> {
        let (moment, local) = if let Some(moment) = self.at_local_datetime() {
            (moment, true)
        } else {
            let Tok::KeyWord(word @ (kw::CURRENT_DATE | kw::CURRENT_TIME | kw::CURRENT_TIMESTAMP)) = *self.peek() else { return Ok(None) };
            (moment_of(word).ok_or_else(|| self.syntax("expected a <datetime value function>"))?, false)
        };
        self.advance();
        let precision = if moment != Moment::Date && self.accept_delim(delim::LEFT_PAREN) {
            let p = self.time_precision()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            Some(p)
        } else {
            None
        };
        Ok(Some(DatetimeFunction { moment, precision, local }))
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
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => return Err(self.syntax("expected an <extract field>")),
        };
        self.advance();
        self.expect_kw(kw::FROM)?;
        let source = Box::new(self.common_value_expression()?);
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Extract { field, source, at })
    }

    /// [ISO/IEC 9075:1992 §6.1] `<datetime type>` ::= DATE | TIME
    /// \[(`<time precision>`)\] \[`<with or without time zone>`\] |
    /// TIMESTAMP \[(`<timestamp precision>`)\] \[`<with or without time
    /// zone>`\]; of an omitted precision 0 for TIME and 6 for TIMESTAMP
    /// [SR25]. [ISO/IEC 9075-2:2023 §6.1] `<with or without time zone>`
    /// ::= WITH TIME ZONE | WITHOUT TIME ZONE, WITHOUT implied [SR39].
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
        if zoned || self.accept_word(WITHOUT) {
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
