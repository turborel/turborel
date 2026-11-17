//! The item methods that cast an SQL/JSON item to an SQL type [ISO/IEC
//! 9075-2:2023 §9.46 GR11g ii 6 C IX to XIX]: `bigint()`, `boolean()`,
//! `date()`, `decimal()`, `integer()`, `number()`, `string()`, `time()`,
//! `time_tz()`, `timestamp()` and `timestamp_tz()`, Features T865 to T878.
//!
//! Each takes items of some kinds alone, each other item the data
//! exception its rule names; of an item it takes, its value is CAST to the
//! type, an exception of the cast the path's \[GR11g ii 6 C IX 2 a and the
//! like\]. Implementation-defined choices: (IV186) the numeric type of
//! `number()` is, of a number, its own, so a number is kept as it is, and
//! of a character string the type of the `<signed numeric literal>` it is,
//! exact or approximate, as CAST reads one [ISO/IEC 9075:1992 §6.10
//! GR3b, GR4b]; (IL006) the
//! maximum length of `string()`'s CHARACTER VARYING is turborel's, 32 767
//! characters. \[DESIGN\] A cast to a datetime, or from a datetime of a
//! type without time zone, takes the SQL-session's displacement to be UTC
//! and its date 0001-01-01, as `cast` does: the evaluator has no
//! SQL-session.
//!
//! As printed, `time_tz()` and `timestamp_tz()` take "a number or
//! character string" \[XVII 1 a, XIX 1 a\], where `time()` and
//! `timestamp()` take a character string alone. That is followed: a
//! number is cast, and CAST takes none to a datetime, so it is SQLSTATE
//! 22005 [ISO/IEC 9075:1992 §6.10 SR4].

use super::super::{exception, item};
use crate::error::{Result, SqlState};
use crate::json::Item;
use crate::value::cast::numeric_literal;
use crate::value::{BIGINT_PRECISION, DEFAULT_EXACT_PRECISION, DataType, INTEGER_PRECISION, MAX_CHARACTER_LENGTH, Spelled, Value, cast};

/// [ISO/IEC 9075-2:2023 §6.1 SR38] The precision of a TIME of none
/// written.
const DEFAULT_TIME_PRECISION: u32 = 0;
/// [ISO/IEC 9075-2:2023 §6.1 SR38] The precision of a TIMESTAMP of none
/// written.
const DEFAULT_TIMESTAMP_PRECISION: u32 = 6;
/// [ISO/IEC 9075-2:2023 §6.1 SR24] The scale of a DECIMAL of none written.
const DEFAULT_SCALE: u32 = 0;

/// The kind of an SQL/JSON scalar, as the methods take them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A number: exact, approximate or decimal floating-point.
    Number,
    /// A character string.
    String,
    /// A truth value.
    Boolean,
    /// A datetime, which no method of these takes.
    Datetime,
}

/// What the cast is to.
#[derive(Debug, Clone)]
enum Target {
    /// The SQL type.
    Type(DataType),
    /// `number()`'s implementation-defined numeric type (IV186).
    Number,
}

/// An item method that casts: the kinds of item it takes, the exception
/// of any other, the rule that raises it, its name, and what it casts to.
#[derive(Debug, Clone)]
pub(super) struct Conversion {
    takes: &'static [Kind],
    refusal: SqlState,
    rule: &'static str,
    name: &'static str,
    to: Target,
}

/// The kinds `bigint()`, `decimal()`, `integer()` and `number()` take, and
/// `time_tz()` and `timestamp_tz()` as printed.
const NUMBER_OR_STRING: &[Kind] = &[Kind::Number, Kind::String];
/// The kind `date()`, `time()` and `timestamp()` take.
const STRING_ONLY: &[Kind] = &[Kind::String];

/// [§9.46 GR11g ii 6 C IX] `bigint()`: CAST (X AS BIGINT).
pub(super) const BIGINT: Conversion = Conversion {
    takes: NUMBER_OR_STRING,
    refusal: SqlState::NON_NUMERIC_SQL_JSON_ITEM,
    rule: "GR11g ii 6 C IX 1 a",
    name: "bigint()",
    to: Target::Type(DataType::Exact { precision: BIGINT_PRECISION, scale: 0, spelled: Spelled::Bigint }),
};

/// [§9.46 GR11g ii 6 C X] `boolean()`: CAST (X AS BOOLEAN), of a truth
/// value or a character string.
pub(super) const BOOLEAN: Conversion = Conversion {
    takes: &[Kind::Boolean, Kind::String],
    refusal: SqlState::NON_BOOLEAN_SQL_JSON_ITEM,
    rule: "GR11g ii 6 C X 1 a",
    name: "boolean()",
    to: Target::Type(DataType::Boolean),
};

/// [§9.46 GR11g ii 6 C XI] `date()`: CAST (X AS DATE), of a character
/// string.
pub(super) const DATE: Conversion = Conversion {
    takes: STRING_ONLY,
    refusal: SqlState::NON_DATE_SQL_JSON_ITEM,
    rule: "GR11g ii 6 C XI 1 a",
    name: "date()",
    to: Target::Type(DataType::Date),
};

/// [§9.46 GR11g ii 6 C XIII] `integer()`: CAST (X AS INTEGER).
pub(super) const INTEGER: Conversion = Conversion {
    takes: NUMBER_OR_STRING,
    refusal: SqlState::NON_NUMERIC_SQL_JSON_ITEM,
    rule: "GR11g ii 6 C XIII 1 a",
    name: "integer()",
    to: Target::Type(DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer }),
};

/// [§9.46 GR11g ii 6 C XIV] `number()`: CAST (X AS NT), NT the numeric
/// type of IV186.
pub(super) const NUMBER: Conversion = Conversion {
    takes: NUMBER_OR_STRING,
    refusal: SqlState::NON_NUMERIC_SQL_JSON_ITEM,
    rule: "GR11g ii 6 C XIV 1 a",
    name: "number()",
    to: Target::Number,
};

/// [§9.46 GR11g ii 6 C XV] `string()`: CAST (X AS CHARACTER VARYING(ML)),
/// of a character string, a number or a truth value.
pub(super) const STRING: Conversion = Conversion {
    takes: &[Kind::String, Kind::Number, Kind::Boolean],
    refusal: SqlState::NON_STRING_SQL_JSON_ITEM,
    rule: "GR11g ii 6 C XV 1 a",
    name: "string()",
    to: Target::Type(DataType::varying(MAX_CHARACTER_LENGTH)),
};

/// [§9.46 GR11g ii 6 C XII] `decimal()`: CAST (X AS DECIMAL PS), PS the
/// `precision` and `scale` written, of none the defaults [§6.1 SR22,
/// SR24].
pub(super) fn decimal(precision: Option<u32>, scale: Option<u32>) -> Conversion {
    Conversion {
        takes: NUMBER_OR_STRING,
        refusal: SqlState::NON_NUMERIC_SQL_JSON_ITEM,
        rule: "GR11g ii 6 C XII 1 a",
        name: "decimal()",
        to: Target::Type(DataType::Exact {
            precision: precision.unwrap_or(DEFAULT_EXACT_PRECISION),
            scale: scale.unwrap_or(DEFAULT_SCALE),
            spelled: Spelled::Decimal,
        }),
    }
}

/// [§9.46 GR11g ii 6 C XVI] `time()`: CAST (X AS TIME PS), of a character
/// string.
pub(super) fn time(precision: Option<u32>) -> Conversion {
    Conversion {
        takes: STRING_ONLY,
        refusal: SqlState::NON_TIME_SQL_JSON_ITEM,
        rule: "GR11g ii 6 C XVI 1 a",
        name: "time()",
        to: Target::Type(DataType::Time { precision: precision.unwrap_or(DEFAULT_TIME_PRECISION), zoned: false }),
    }
}

/// [§9.46 GR11g ii 6 C XVII] `time_tz()`: CAST (X AS TIME PS WITH TIME
/// ZONE).
pub(super) fn time_tz(precision: Option<u32>) -> Conversion {
    Conversion {
        takes: NUMBER_OR_STRING,
        refusal: SqlState::NON_TIME_SQL_JSON_ITEM,
        rule: "GR11g ii 6 C XVII 1 a",
        name: "time_tz()",
        to: Target::Type(DataType::Time { precision: precision.unwrap_or(DEFAULT_TIME_PRECISION), zoned: true }),
    }
}

/// [§9.46 GR11g ii 6 C XVIII] `timestamp()`: CAST (X AS TIMESTAMP PS), of
/// a character string.
pub(super) fn timestamp(precision: Option<u32>) -> Conversion {
    Conversion {
        takes: STRING_ONLY,
        refusal: SqlState::NON_TIMESTAMP_SQL_JSON_ITEM,
        rule: "GR11g ii 6 C XVIII 1 a",
        name: "timestamp()",
        to: Target::Type(DataType::Timestamp { precision: precision.unwrap_or(DEFAULT_TIMESTAMP_PRECISION), zoned: false }),
    }
}

/// [§9.46 GR11g ii 6 C XIX] `timestamp_tz()`: CAST (X AS TIMESTAMP PS WITH
/// TIME ZONE).
pub(super) fn timestamp_tz(precision: Option<u32>) -> Conversion {
    Conversion {
        takes: NUMBER_OR_STRING,
        refusal: SqlState::NON_TIMESTAMP_SQL_JSON_ITEM,
        rule: "GR11g ii 6 C XIX 1 a",
        name: "timestamp_tz()",
        to: Target::Type(DataType::Timestamp { precision: precision.unwrap_or(DEFAULT_TIMESTAMP_PRECISION), zoned: true }),
    }
}

/// [§9.46 GR11g ii 6 C IX to XIX] Each item of `base` cast as `conversion`
/// says.
///
/// # Errors
///
/// The exception of `conversion` if an item is of no kind it takes; else
/// the cast's, as 22018 of a string that is no literal of the type, 22003
/// of a number out of its range.
pub(super) fn converted(base: &[Item], conversion: &Conversion) -> Result<Vec<Item>> {
    base.iter().map(|each| conversion.of(each)).collect()
}

impl Conversion {
    /// `item` cast.
    fn of(&self, each: &Item) -> Result<Item> {
        let taken = each.as_scalar().filter(|scalar| self.takes.contains(&kind(scalar.value())));
        let Some(scalar) = taken else {
            return Err(exception(
                self.refusal,
                &format!("[ISO/IEC 9075-2:2023 §9.46 {}] an SQL/JSON item of {} is not of a kind it takes", self.rule, self.name),
            ));
        };
        let value = scalar.value().clone();
        match &self.to {
            Target::Type(to) => cast(value, to).map(item),
            Target::Number => number(value).map(item),
        }
    }
}

/// (IV186) `value`, a number or a character string, CAST to `number()`'s
/// type: a number as it is, a string as the `<signed numeric literal>` it
/// is.
///
/// # Errors
///
/// SQLSTATE 22018 if a string is none [ISO/IEC 9075:1992 §6.10 GR3b i,
/// GR4b i].
fn number(value: Value) -> Result<Value> {
    if let Value::Char(text) = &value { numeric_literal(text) } else { Ok(value) }
}

/// The kind of `value`, an SQL/JSON scalar's.
fn kind(value: &Value) -> Kind {
    match value {
        Value::Char(_) => Kind::String,
        Value::Bool(_) => Kind::Boolean,
        Value::Datetime(_) => Kind::Datetime,
        // A scalar of none of the types above is a number (`Scalar::new`).
        Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Null
        | Value::Binary(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Extension(_) => Kind::Number,
    }
}

#[cfg(test)]
mod tests;
