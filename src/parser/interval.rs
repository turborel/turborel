//! [ISO/IEC 9075:1992 §5.3, §6.14, §6.15, §10.1, §16.5] The
//! interval grammar: the `<interval qualifier>`, the `<interval type>`
//! and `<interval literal>`, the difference of two datetimes in an
//! interval, `<time zone>`, and SET TIME ZONE. Split from parser.rs
//! (RULES.md R-SIZE-2); the methods extend the same `Parser`.

use super::Parser;
use crate::ast::{Literal, SqlStatement, TypeName, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::value::DataType;
use crate::value::datetime::{Field, MAX_TIME_PRECISION};
use crate::value::interval::{
    DEFAULT_FRACTION_PRECISION, DEFAULT_LEADING_PRECISION, Interval, MAX_LEADING_PRECISION, Qualifier, field_name, parse, rank,
};

/// The `<datetime field>` the key word `word` names.
fn field_of(word: &str) -> Option<Field> {
    match word {
        kw::YEAR => Some(Field::Year),
        kw::MONTH => Some(Field::Month),
        kw::DAY => Some(Field::Day),
        kw::HOUR => Some(Field::Hour),
        kw::MINUTE => Some(Field::Minute),
        kw::SECOND => Some(Field::Second),
        _ => None,
    }
}

impl Parser {
    /// Whether an `<interval qualifier>` is next: a `<datetime field>`.
    pub(super) fn at_interval_qualifier(&self) -> bool {
        if let Tok::KeyWord(word) = self.peek() { field_of(word).is_some() } else { false }
    }

    /// The `<datetime field>` next, taken.
    fn datetime_field(&mut self) -> Result<Field> {
        let Tok::KeyWord(word) = *self.peek() else { return Err(self.syntax("expected a <datetime field>")) };
        let field = field_of(word).ok_or_else(|| self.syntax("expected a <datetime field>"))?;
        self.advance();
        Ok(field)
    }

    /// A precision in parentheses, if one is next: of the leading field
    /// more than 0 [§10.1 SR5]; each no more than this implementation's.
    fn interval_precision(&mut self, leading: bool) -> Result<u32> {
        let at = self.here();
        let n = self.unsigned_integer()?;
        let (most, what) = if leading {
            (MAX_LEADING_PRECISION, "<interval leading field precision>")
        } else {
            (MAX_TIME_PRECISION, "<interval fractional seconds precision>")
        };
        if leading && n == 0 {
            return Err(SqlError::at(
                Code::SyntaxRule,
                at,
                "[ISO/IEC 9075:1992 §10.1 SR5] an <interval leading field precision> is more than 0",
            ));
        }
        if n > most {
            return Err(SqlError::at(
                Code::Unsupported,
                at,
                format!("[DESIGN] a {what} of {n} is more than this implementation's maximum of {most}"),
            ));
        }
        Ok(n)
    }

    /// [ISO/IEC 9075:1992 §10.1] `<interval qualifier>` ::= `<start field>`
    /// TO `<end field>` | `<single datetime field>`: its start more
    /// significant than its end, not MONTH, and YEAR only TO MONTH [SR2];
    /// of a leading precision 2 and a fraction's 6 if none is written
    /// [SR5, SR6].
    pub(super) fn interval_qualifier(&mut self) -> Result<Qualifier> {
        let at = self.here();
        let start = self.datetime_field()?;
        let (mut leading, mut fraction) = (DEFAULT_LEADING_PRECISION, DEFAULT_FRACTION_PRECISION);
        if self.accept_delim(delim::LEFT_PAREN) {
            leading = self.interval_precision(true)?;
            if start == Field::Second && self.accept_delim(delim::COMMA) {
                fraction = self.interval_precision(false)?;
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        if start == Field::Second || !self.accept_kw(kw::TO) {
            return Ok(Qualifier::new(start, start, leading, fraction));
        }
        let end = self.datetime_field()?;
        if end == Field::Second && self.accept_delim(delim::LEFT_PAREN) {
            fraction = self.interval_precision(false)?;
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        let valid = rank(start) < rank(end) && start != Field::Month && (start != Field::Year || end == Field::Month);
        if !valid {
            let msg = format!("[ISO/IEC 9075:1992 §10.1 SR2] {} TO {} is no <interval qualifier>", field_name(start), field_name(end));
            return Err(SqlError::at(Code::SyntaxRule, at, msg));
        }
        Ok(Qualifier::new(start, end, leading, fraction))
    }

    /// Whether an `<interval literal>` is next: INTERVAL, a sign, if one,
    /// and a character string.
    pub(super) fn at_interval_literal(&self) -> bool {
        let string = |t: &Tok| matches!(t, Tok::CharacterString(_));
        self.is_kw(kw::INTERVAL)
            && (string(self.peek_at(1))
                || (matches!(self.peek_at(1), Tok::Delimiter(delim::PLUS | delim::MINUS)) && string(self.peek_at(2))))
    }

    /// [ISO/IEC 9075:1992 §5.3] `<interval literal>` ::= INTERVAL \[`<sign>`\]
    /// `<interval string>` `<interval qualifier>`, its string of the fields
    /// the qualifier gives [SR20], each in its range [SR23], the leading one
    /// of no more digits than its precision [§10.1 SR5].
    pub(super) fn interval_literal(&mut self) -> Result<Literal> {
        let at = self.here();
        self.expect_kw(kw::INTERVAL)?;
        let negative = self.accept_delim(delim::MINUS);
        if !negative {
            self.accept_delim(delim::PLUS);
        }
        let Tok::CharacterString(text) = self.peek().clone() else { return Err(self.syntax("expected an <interval string>")) };
        self.advance();
        let qualifier = self.interval_qualifier()?;
        let Some(units) = parse(&qualifier, &text) else {
            let msg = format!("[ISO/IEC 9075:1992 §5.3 SR20, SR23; §10.1 SR5] '{text}' is no <interval string> of {qualifier}");
            return Err(SqlError::at(Code::SyntaxRule, at, msg));
        };
        let units = if negative { units.saturating_neg() } else { units };
        Ok(Literal::Interval(Interval { units, qualifier }))
    }

    /// After a `<value expression primary>` `primary`: [§6.15]
    /// (`<datetime value expression>` - `<datetime term>`) `<interval
    /// qualifier>`, if a qualifier follows a difference in parentheses; a
    /// `<dynamic parameter specification>` with an `<interval qualifier>`,
    /// of that type [SR2; §17.6 GR5a]; else `primary`.
    pub(super) fn qualified(&mut self, primary: ValueExpression, at: usize) -> Result<ValueExpression> {
        if !self.at_interval_qualifier() {
            return Ok(primary);
        }
        if let ValueExpression::Nested(inner) = primary {
            let ValueExpression::Dyadic(minuend, crate::value::ArithOp::Sub, subtrahend) = *inner else {
                return Err(self.syntax("an <interval qualifier> follows a difference of datetimes in parentheses"));
            };
            let qualifier = self.interval_qualifier()?;
            return Ok(ValueExpression::Difference { minuend, subtrahend, qualifier, at });
        }
        if matches!(primary, ValueExpression::Value(crate::ast::ValueSpecification::Dynamic { .. })) {
            let target = TypeName::Data(DataType::Interval(self.interval_qualifier()?));
            return Ok(ValueExpression::Cast { operand: Some(Box::new(primary)), target, at });
        }
        Ok(primary)
    }

    /// [ISO/IEC 9075:1992 §6.14] `<datetime factor>` ::= `<datetime
    /// primary>` \[`<time zone>`\], `<time zone>` ::= AT {LOCAL | TIME ZONE
    /// `<interval value expression>`}: `primary` with its time zone, if
    /// one follows.
    pub(super) fn time_zone(&mut self, primary: ValueExpression) -> Result<ValueExpression> {
        let at = self.here();
        if !self.accept_kw(kw::AT) {
            return Ok(primary);
        }
        let zone = if self.accept_kw(kw::LOCAL) {
            None
        } else {
            self.expect_kw(kw::TIME)?;
            self.expect_kw(kw::ZONE)?;
            Some(Box::new(self.factor()?))
        };
        Ok(ValueExpression::AtZone { source: Box::new(primary), zone, at })
    }

    /// [ISO/IEC 9075:1992 §16.5] `<set local time zone statement>` ::= SET
    /// TIME ZONE {`<interval value expression>` | LOCAL}, after SET.
    pub(super) fn set_time_zone(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::TIME)?;
        self.expect_kw(kw::ZONE)?;
        if self.accept_kw(kw::LOCAL) {
            return Ok(SqlStatement::SetTimeZone(None));
        }
        Ok(SqlStatement::SetTimeZone(Some(self.value_expression()?)))
    }
}
