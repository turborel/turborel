//! The item method `datetime()` evaluated [ISO/IEC 9075-2:2023 §9.46
//! GR11g ii 6 C VII]: a character string read as the unquoted string of a
//! date, time or timestamp literal [§5.3], tried as a DATE, a TIME WITH
//! TIME ZONE, a TIME WITHOUT TIME ZONE, a TIMESTAMP WITH TIME ZONE and a
//! TIMESTAMP WITHOUT TIME ZONE in turn \[GR11g ii 6 C VII 1 c i to v\].
//!
//! A string is of a type WITH TIME ZONE if it writes a `<time zone
//! interval>`, and of one WITHOUT if it does not: the rule tries TIME WITH
//! TIME ZONE first, but a literal without a `<time zone interval>` is of a
//! type without time zone [§5.3 SR34, SR35], so it is not taken to determine a
//! value of the type with one. Its precision is the digits of its
//! `<seconds fraction>` [§5.3 SR34, SR35]. A datetime without time zone
//! is taken in the SQL-session's time zone displacement, as a literal is
//! and as a template's conversion takes it [§9.51 GR]: \[DESIGN\] the
//! displacement as the path was parsed, which `Path::zone` keeps, as the
//! template's conversion keeps it. Of more than 6 digits of a second's
//! fraction, turborel's most [§6.1 SR40], a string is SQLCODE -900
//! (RULES.md D18).
//!
//! (UA070) Of several items in error, the first is the exception raised.
//!
//! With a `<JSON datetime template>`, a string is read by the General
//! Rules of §9.51 as a value of the template's implicit datetime data type
//! \[VII 1 b\], by the conversion made of the template as the path was
//! parsed (`DatetimeTemplate`); an exception they raise is SQLSTATE 22031
//! instead.

use std::borrow::Cow;

use super::super::{exception, item, string};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::json::Item;
use crate::json::path::ast::DatetimeTemplate;
use crate::value::Value;
use crate::value::datetime::{Datetime, MAX_TIME_PRECISION, Moment, parse};

/// [§9.46 GR11g ii 6 C VII 1 c i to v] The datetime types a string is
/// tried as, in order: of which fields, and whether WITH TIME ZONE.
const INTERPRETATIONS: [(Moment, bool); 5] =
    [(Moment::Date, false), (Moment::Time, true), (Moment::Time, false), (Moment::Timestamp, true), (Moment::Timestamp, false)];

/// [§9.46 GR11g ii 6 C VII 1 a] The exception of an item that is no
/// character string.
const NOT_A_STRING: &str = "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 a] an SQL/JSON item of datetime() is not a character string";

/// [§9.46 GR11g ii 6 C VII] `datetime()` of each item of `base`, one of a
/// type without time zone taken in the displacement `zone`, in minutes.
///
/// # Errors
///
/// SQLSTATE 22031 if an item is no character string \[VII 1 a\], or one
/// that writes no datetime of any of the five types \[VII 1 c vi\]; SQLCODE
/// -900 of one of more digits of a second's fraction than turborel's
/// maximum.
pub(super) fn datetime(base: &[Cow<'_, Item>], zone: i16) -> Result<Vec<Item>> {
    base.iter()
        .map(|each| {
            let Some(text) = string(each) else { return Err(invalid(NOT_A_STRING)) };
            interpreted(text, zone)
        })
        .collect()
}

/// [§9.46 GR11g ii 6 C VII 1 a, b] `datetime()` of `template` of each item
/// of `base`: the DATETIME VALUE of §9.51's General Rules, of the
/// template's implicit datetime data type.
///
/// # Errors
///
/// SQLSTATE 22031 if an item is no character string \[VII 1 a\], or one
/// of which §9.51's General Rules raise an exception condition \[VII 1
/// b\].
pub(super) fn templated(base: &[Cow<'_, Item>], template: &DatetimeTemplate) -> Result<Vec<Item>> {
    base.iter()
        .map(|each| {
            let Some(text) = string(each) else { return Err(invalid(NOT_A_STRING)) };
            let read = template.cast().call(&[Value::Char(text.into())]);
            read.map(item).map_err(|e| if e.code == Code::Data { unread(text, template, &e) } else { e })
        })
        .collect()
}

/// [§9.46 GR11g ii 6 C VII 1 b] The exception that replaces `raised`, the
/// exception §9.51 raises of `text` by `template`.
fn unread(text: &str, template: &DatetimeTemplate, raised: &SqlError) -> SqlError {
    invalid(&format!(
        "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 b] the SQL/JSON item '{text}' of datetime() is not read by the template '{}': {}",
        template.text(),
        raised.msg
    ))
}

/// The datetime `text` writes, as the first of `INTERPRETATIONS` it is a
/// valid value of, in the displacement `zone` if it writes none.
fn interpreted(text: &str, zone: i16) -> Result<Item> {
    for (moment, zoned) in INTERPRETATIONS {
        let Some(written) = parse(moment, text) else { continue };
        if written.zone.is_some() != zoned {
            continue;
        }
        if written.precision > MAX_TIME_PRECISION {
            return Err(SqlError::new(
                Code::Unsupported,
                format!(
                    "[DESIGN] the SQL/JSON item '{text}' of datetime() has more digits of the second than {MAX_TIME_PRECISION} (RULES.md D18)"
                ),
            ));
        }
        let value = Datetime::of_local(moment, written.local, written.zone.unwrap_or(zone), zoned, written.precision);
        // [§6.1 GR6] A valid value is of a year from 0001 to 9999.
        if value.in_range() {
            return Ok(item(Value::Datetime(value)));
        }
    }
    Err(invalid(&format!(
        "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 c vi] the SQL/JSON item '{text}' of datetime() is no valid date, time or timestamp"
    )))
}

/// The exception of `datetime()`, its message `cited`.
fn invalid(cited: &str) -> SqlError {
    exception(SqlState::INVALID_ARGUMENT_FOR_SQL_JSON_DATETIME_FUNCTION, cited)
}

#[cfg(test)]
mod tests;
