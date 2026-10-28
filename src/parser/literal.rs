//! The `<value specification>` and `<literal>` grammar [X3.135-1986 §5.2,
//! §5.6]: USER, character strings and exact and approximate numbers, with this
//! implementation's limits on them (RULES.md D18). Split from parser.rs (RULES.md
//! R-SIZE-2).

use super::Parser;
use crate::ast::{Literal, UserFunction, ValueSpecification};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::value::{Decimal, MAX_EXACT_PRECISION};

impl Parser {
    /// [X3.135-1986 §5.6] `<value specification>` ::= `<literal>` | USER
    /// | CURRENT_USER | SESSION_USER | SYSTEM_USER [ISO/IEC 9075:1992 §6.2]
    pub(super) fn value_specification(&mut self) -> Result<ValueSpecification> {
        if let Some(user) = self.user_function() {
            return Ok(ValueSpecification::User(user));
        }
        if self.at_parameter() {
            return Ok(ValueSpecification::Parameter(self.parameter_specification()?));
        }
        if self.is_delim(delim::COLON) {
            return Err(self.unsupported("<variable specification> (embedded SQL)"));
        }
        if self.is_delim(delim::QUESTION) {
            // [ISO/IEC 9075:1992 §6.2 SR2] only in a statement being prepared.
            let Some(index) = self.dynamic else {
                return Err(self.rule("[ISO/IEC 9075:1992 §6.2 SR2] a <dynamic parameter specification> is in a prepared statement only"));
            };
            let at = self.here();
            self.advance();
            self.dynamic = Some(index.saturating_add(1));
            return Ok(ValueSpecification::Dynamic { index, at });
        }
        Ok(ValueSpecification::Literal(self.literal()?))
    }

    /// [ISO/IEC 9075:1992 §6.2] USER, CURRENT_USER, SESSION_USER or
    /// SYSTEM_USER, taken, if one is next.
    pub(super) fn user_function(&mut self) -> Option<UserFunction> {
        let user = match self.peek() {
            // [SR3] USER is CURRENT_USER.
            Tok::KeyWord(kw::USER) | Tok::KeyWord(kw::CURRENT_USER) => UserFunction::Current,
            Tok::KeyWord(kw::SESSION_USER) => UserFunction::Session,
            Tok::KeyWord(kw::SYSTEM_USER) => UserFunction::System,
            Tok::KeyWord(_)
            | Tok::Identifier(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Introducer(_)
            | Tok::Delimiter(_)
            | Tok::End => return None,
        };
        self.advance();
        Some(user)
    }

    /// Is the current "+" or "-" written directly against a numeric literal?
    /// [X3.135-1986 §5.2] makes the sign part of an `<exact numeric literal>`, and [X3.135-1986 §5.3 SR1] says
    /// a token contains no `<space>`, so "-5" is one literal and "- 5" is not.
    pub(super) fn sign_attached(&self) -> bool {
        let (Some(sign), Some(number)) = (self.toks.get(self.pos), self.toks.get(self.pos.saturating_add(1))) else {
            return false;
        };
        matches!(sign.tok, Tok::Delimiter(delim::PLUS) | Tok::Delimiter(delim::MINUS))
            && matches!(number.tok, Tok::ExactNumeric(_) | Tok::ApproximateNumeric(_))
            && number.start == sign.end
    }

    /// `<literal>` ::= `<character string literal>` | `<numeric literal>` |
    /// `<bit string literal>` | `<hex string literal>` [ISO/IEC 9075:1992
    /// §5.3]
    pub(super) fn literal(&mut self) -> Result<Literal> {
        if self.at_datetime_literal() {
            return self.datetime_literal();
        }
        if self.at_interval_literal() {
            return self.interval_literal();
        }
        // [ISO/IEC 9075:1992 §5.3] <introducer><character set specification>
        // before a <character string literal>.
        if matches!(self.peek(), Tok::Introducer(_)) && matches!(self.peek_at(1), Tok::CharacterString(_)) {
            self.introducer();
        }
        let mut negative = false;
        if self.sign_attached() {
            negative = self.is_delim(delim::MINUS);
            self.advance();
        }
        let at = self.here();
        let lit = match self.peek().clone() {
            Tok::CharacterString(s) => Literal::Character(s),
            Tok::BitString(b) => Literal::Bit(b),
            Tok::ExactNumeric(t) => {
                // [X3.135-1986 §5.2 SR4] sets no maximum number of digits (RULES.md D18).
                let d = parse_exact(&t).map_err(|m| SqlError::at(Code::Unsupported, at, m))?;
                Literal::Exact(if negative { d.negated() } else { d })
            }
            Tok::ApproximateNumeric(t) => {
                let f = parse_approximate(&t, at)?;
                Literal::Approximate(if negative { -f } else { f })
            }
            Tok::KeyWord(_) | Tok::Identifier(_) | Tok::Introducer(_) | Tok::Delimiter(_) | Tok::End => {
                return Err(self.syntax("expected a <literal>"));
            }
        };
        self.advance();
        Ok(lit)
    }
}

/// [X3.135-1986 §5.2 GR2, SR3, SR4] the value, precision and scale of an `<exact numeric literal>`.
fn parse_exact(text: &str) -> std::result::Result<Decimal, String> {
    let (int, frac) = text.split_once('.').unwrap_or((text, ""));
    // [X3.135-1986 §5.2 SR4] precision is the number of <digit>s it contains.
    let digits = u32::try_from(int.len().saturating_add(frac.len())).unwrap_or(u32::MAX);
    if digits > MAX_EXACT_PRECISION {
        return Err(format!(
            "[X3.135-1986 §5.2 SR4] <exact numeric literal> {text} has {digits} digits; this implementation's maximum exact precision is {MAX_EXACT_PRECISION}"
        ));
    }
    // At most MAX_EXACT_PRECISION digits, which the lexer guarantees are
    // digits, always fit an i128, so neither fallback below is ever taken.
    let units: i128 = format!("{int}{frac}").parse().unwrap_or(0);
    let scale = u32::try_from(frac.len()).unwrap_or(MAX_EXACT_PRECISION);
    Ok(Decimal::new(units, scale))
}

/// [X3.135-1986 §5.2 GR3] mantissa times 10 to the exponent, for the
/// `<approximate numeric literal>` `text` at character `at`.
fn parse_approximate(text: &str, at: usize) -> Result<f64> {
    let Some((mantissa, exponent)) = text.split_once('E') else {
        return Err(SqlError::at(Code::Syntax, at, format!("[X3.135-1986 §5.2 F] {text} is not an <approximate numeric literal>")));
    };
    // Give "5." and ".5" the digits Rust's float parser wants on both sides.
    let mantissa = if mantissa.contains('.') { format!("0{mantissa}0") } else { mantissa.to_string() };
    format!("{mantissa}e{exponent}").parse::<f64>().ok().filter(|f| f.is_finite()).ok_or_else(|| {
        SqlError::at(
            Code::Unsupported,
            at,
            format!("[X3.135-1986 §5.2 SR5] {text} is outside the range of this implementation's approximate numbers"),
        )
    })
}
