//! The `<JSON method>` of a `<JSON item method>` parsed [ISO/IEC
//! 9075-2:2023 §9.46 F]: its name, and what its parentheses hold: the
//! `<precision>` and `<scale>` of `decimal()`, the `<time precision>` or
//! `<timestamp precision>` of `time()`, `time_tz()`, `timestamp()` and
//! `timestamp_tz()`, and the `<JSON datetime template>` of `datetime()`
//! \[SR10\].
//!
//! A precision or scale is an `<unsigned integer>` [§5.3], a numeric
//! literal of its digits alone (`Numeral::Integer`), and is held to the
//! Syntax Rules of the SQL type it is written for [§6.1 SR23, SR25, SR27,
//! SR40], as `decimal(p, s)` casts to DECIMAL(p, s) \[GR11g ii 6 C XII\]:
//! a precision of 0, or a scale above the precision, is SQLCODE -103; one
//! past turborel's maximum, 18 digits of an exact number or 6 of a
//! second's fraction, is SQLCODE -900 (RULES.md D18).

use super::Parser;
use crate::error::Result;
use crate::functions::json_datetime_cast;
use crate::json::path::ast::{DatetimeTemplate, Method};
use crate::json::path::lex::{Numeral, Symbol, Token};
use crate::json::path::{
    ABS, BIGINT, BOOLEAN, CEILING, DATE, DATETIME, DECIMAL, DOUBLE, FLOOR, INTEGER, KEYVALUE, NUMBER, SIZE, STRING, TIME, TIME_TZ,
    TIMESTAMP, TIMESTAMP_TZ, TYPE, beyond, misplaced, place, syntax,
};
use crate::json::{Item, Scalar};
use crate::value::datetime::MAX_TIME_PRECISION;
use crate::value::{MAX_EXACT_PRECISION, Value};

/// [§9.46 F] The `<JSON method>`s whose parentheses hold nothing, by
/// name.
const METHODS: [(&str, Method); 13] = [
    (TYPE, Method::Type),
    (SIZE, Method::Size),
    (DOUBLE, Method::Double),
    (CEILING, Method::Ceiling),
    (FLOOR, Method::Floor),
    (ABS, Method::Abs),
    (KEYVALUE, Method::Keyvalue),
    (BIGINT, Method::Bigint),
    (BOOLEAN, Method::Boolean),
    (DATE, Method::Date),
    (INTEGER, Method::Integer),
    (NUMBER, Method::Number),
    (STRING, Method::String),
];

/// The method of a precision written or not.
type OfPrecision = fn(Option<u32>) -> Method;

/// [§9.46 F] The `<JSON method>`s whose parentheses may hold a `<time
/// precision>` or a `<timestamp precision>`, by name, each with the method
/// of the precision written or not.
const PRECISIONED: [(&str, OfPrecision); 4] =
    [(TIME, Method::Time), (TIME_TZ, Method::TimeTz), (TIMESTAMP, Method::Timestamp), (TIMESTAMP_TZ, Method::TimestampTz)];

impl Parser<'_> {
    /// The `<JSON method>` named `name`, at character `at`, its `(` read:
    /// what its parentheses hold, and its `)`.
    pub(super) fn method(&mut self, name: &str, at: usize) -> Result<Method> {
        let method = if let Some((_, method)) = METHODS.iter().find(|(word, _)| *word == name) {
            method.clone()
        } else if let Some((_, of)) = PRECISIONED.iter().find(|(word, _)| *word == name) {
            of(self.fraction_precision(name)?)
        } else if name == DECIMAL {
            self.decimal()?
        } else if name == DATETIME {
            self.datetime()?
        } else {
            return Err(syntax(at, "a <JSON method>", name));
        };
        self.expect(Symbol::RightParen)?;
        Ok(method)
    }

    /// The `<unsigned integer>` next, taken, and where it begins; none if
    /// no number is next. \[DESIGN\] Of more digits than a `u32` holds it is
    /// `u32::MAX`, past every maximum.
    fn unsigned(&mut self) -> Result<Option<(u32, usize)>> {
        let at = self.here();
        let Some(Token::Number(item, numeral)) = self.peek() else { return Ok(None) };
        if *numeral != Numeral::Integer {
            return Err(self.expected("an <unsigned integer>, of digits alone"));
        }
        let value = whole(item);
        self.bump();
        Ok(Some((value, at)))
    }

    /// The parentheses of `decimal` but their `)`: nothing, a
    /// `<precision>`, or a `<precision>`, `,` and a `<scale>`.
    fn decimal(&mut self) -> Result<Method> {
        let Some((precision, at)) = self.unsigned()? else { return Ok(Method::Decimal { precision: None, scale: None }) };
        if precision == 0 {
            return Err(misplaced(at, "[ISO/IEC 9075-2:2023 §6.1 SR23] the <precision> of decimal() shall be greater than 0"));
        }
        if precision > MAX_EXACT_PRECISION {
            return Err(beyond(&format!(
                "the <precision> of decimal() is more than {MAX_EXACT_PRECISION}, the most digits of an exact number"
            )));
        }
        if !self.accept(Symbol::Comma) {
            return Ok(Method::Decimal { precision: Some(precision), scale: None });
        }
        let Some((scale, scale_at)) = self.unsigned()? else { return Err(self.expected("a <scale>")) };
        if scale > precision {
            return Err(misplaced(
                scale_at,
                "[ISO/IEC 9075-2:2023 §6.1 SR25] the <scale> of decimal() shall not be greater than its <precision>",
            ));
        }
        Ok(Method::Decimal { precision: Some(precision), scale: Some(scale) })
    }

    /// The parentheses of the method `name`, of a time or a timestamp, but
    /// their `)`: the precision, if written.
    fn fraction_precision(&mut self, name: &str) -> Result<Option<u32>> {
        let Some((precision, _)) = self.unsigned()? else { return Ok(None) };
        if precision > MAX_TIME_PRECISION {
            return Err(beyond(&format!(
                "the precision of {name}() is more than {MAX_TIME_PRECISION}, the most digits of a second's fraction"
            )));
        }
        Ok(Some(precision))
    }

    /// The parentheses of `datetime` but their `)`: nothing, or a `<JSON
    /// datetime template>`, a `<JSON path string literal>`, its value held
    /// to SR10 as it is read and the conversion of §9.51 made of it
    /// (`functions/formatted/implicit.rs`). An error of the template names
    /// where its literal begins.
    fn datetime(&mut self) -> Result<Method> {
        let at = self.here();
        let Some(Token::String(template)) = self.peek() else { return Ok(Method::Datetime) };
        let template = template.clone();
        self.bump();
        let cast = json_datetime_cast(&template, self.now).map_err(|mut e| {
            e.msg = format!("{} {}", e.msg, place(at));
            e
        })?;
        Ok(Method::DatetimeTemplate(DatetimeTemplate::new(template, cast)))
    }
}

/// The value of `item`, the number of an `<unsigned integer>`, as a `u32`:
/// `u32::MAX` if it holds none, as of a DECFLOAT of more than 18 digits.
fn whole(item: &Item) -> u32 {
    let Some(Value::Exact(d)) = item.as_scalar().map(Scalar::value) else { return u32::MAX };
    u32::try_from(d.units).unwrap_or(u32::MAX)
}
