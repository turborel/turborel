//! The item methods of an SQL/JSON path expression evaluated [ISO/IEC
//! 9075-2:2023 §9.46 GR11g ii 6], each applied to every item of its base:
//! `type()`, `size()`, `double()`, `ceiling()`, `floor()` and `abs()`
//! here, `datetime()` in `methods/datetime.rs`, `keyvalue()` in
//! `methods/keyvalue.rs`, and the casts of `bigint()` to `timestamp_tz()`
//! in `methods/casts.rs`. \[GR11g ii 6 A\] In lax mode the base of each
//! but `type()` and `size()` is unwrapped first.

mod casts;
mod datetime;
mod keyvalue;

use super::{Evaluator, Number, count, exception, item, number, numbers};
use crate::error::{Result, SqlState};
use crate::json::Item;
use crate::json::path::ast::Method;
use crate::value::datetime::Moment;
use crate::value::{DOUBLE_PRECISION, DataType, Datetime, Spelled, Value, cast};

// [§9.46 GR11g ii 6 C I 1] The Unicode character strings `type()` gives.
/// Of the SQL/JSON null.
const NULL: &str = "null";
/// Of a number.
const NUMBER: &str = "number";
/// Of a character string.
const STRING: &str = "string";
/// Of a truth value.
const BOOLEAN: &str = "boolean";
/// Of a date.
const DATE: &str = "date";
/// Of a time without time zone.
const TIME: &str = "time without time zone";
/// Of a time with time zone.
const TIME_TZ: &str = "time with time zone";
/// Of a timestamp without time zone.
const TIMESTAMP: &str = "timestamp without time zone";
/// Of a timestamp with time zone.
const TIMESTAMP_TZ: &str = "timestamp with time zone";
/// Of an array.
const ARRAY: &str = "array";
/// Of an object.
const OBJECT: &str = "object";

impl Evaluator<'_> {
    /// [§9.46 GR11g ii 6] `<JSON item method>`: `method` applied to each
    /// item of `base`.
    pub(super) fn method(&self, base: Vec<Item>, method: Method) -> Result<Vec<Item>> {
        match method {
            Method::Type => Ok(base.iter().map(|each| Item::Scalar(type_name(each).to_owned().into())).collect()),
            Method::Size => self.size(&base),
            Method::Double => self.lax_unwrap(base).iter().map(double).collect(),
            Method::Ceiling => numeric(&self.lax_unwrap(base), numbers::ceiling, "GR11g ii 6 C IV 1 a"),
            Method::Floor => numeric(&self.lax_unwrap(base), numbers::floor, "GR11g ii 6 C V 1 a"),
            Method::Abs => numeric(&self.lax_unwrap(base), |n| Ok(numbers::abs(n)), "GR11g ii 6 C VI 1 a"),
            Method::Datetime => datetime::datetime(&self.lax_unwrap(base)),
            Method::Keyvalue => keyvalue::keyvalue(&self.lax_unwrap(base)),
            Method::Bigint => casts::converted(&self.lax_unwrap(base), &casts::BIGINT),
            Method::Boolean => casts::converted(&self.lax_unwrap(base), &casts::BOOLEAN),
            Method::Date => casts::converted(&self.lax_unwrap(base), &casts::DATE),
            Method::Decimal { precision, scale } => casts::converted(&self.lax_unwrap(base), &casts::decimal(precision, scale)),
            Method::Integer => casts::converted(&self.lax_unwrap(base), &casts::INTEGER),
            Method::Number => casts::converted(&self.lax_unwrap(base), &casts::NUMBER),
            Method::String => casts::converted(&self.lax_unwrap(base), &casts::STRING),
            Method::Time(precision) => casts::converted(&self.lax_unwrap(base), &casts::time(precision)),
            Method::TimeTz(precision) => casts::converted(&self.lax_unwrap(base), &casts::time_tz(precision)),
            Method::Timestamp(precision) => casts::converted(&self.lax_unwrap(base), &casts::timestamp(precision)),
            Method::TimestampTz(precision) => casts::converted(&self.lax_unwrap(base), &casts::timestamp_tz(precision)),
        }
    }

    /// [§9.46 GR11g ii 6 C II] `size()`: the number of elements of each
    /// array of `base`, 1 of any other item.
    fn size(&self, base: &[Item]) -> Result<Vec<Item>> {
        // [GR11g ii 6 C II 1] As printed, "any Vj is not an array"; Vj is
        // what the rule then defines, so Ij, the item, is meant.
        if self.strict() && base.iter().any(|each| each.as_array().is_none()) {
            return Err(exception(
                SqlState::SQL_JSON_ARRAY_NOT_FOUND,
                "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C II 1] an SQL/JSON item of size() is not an array",
            ));
        }
        Ok(base.iter().map(|each| count(each.as_array().map_or(1, <[Item]>::len))).collect())
    }
}

/// [§9.46 GR11g ii 6 C I 1] The name `type()` gives of `item`.
fn type_name(item: &Item) -> &'static str {
    match item {
        Item::Null => NULL,
        Item::Scalar(scalar) => scalar_type(scalar.value()),
        Item::Array(_) => ARRAY,
        Item::Object(_) => OBJECT,
    }
}

/// The name `type()` gives of a scalar, the SQL value `value`.
fn scalar_type(value: &Value) -> &'static str {
    match value {
        Value::Char(_) => STRING,
        Value::Bool(_) => BOOLEAN,
        Value::Datetime(datetime) => datetime_type(datetime),
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
        | Value::Extension(_) => NUMBER,
    }
}

/// The name `type()` gives of a datetime, by its fields and whether it is
/// of a type WITH TIME ZONE.
fn datetime_type(datetime: &Datetime) -> &'static str {
    match (datetime.moment, datetime.zoned) {
        (Moment::Date, _) => DATE,
        (Moment::Time, false) => TIME,
        (Moment::Time, true) => TIME_TZ,
        (Moment::Timestamp, false) => TIMESTAMP,
        (Moment::Timestamp, true) => TIMESTAMP_TZ,
    }
}

/// [§9.46 GR11g ii 6 C III] `double()` of `item`: CAST (`item` AS DOUBLE
/// PRECISION), of a number or a character string.
///
/// # Errors
///
/// SQLSTATE 22036 if `item` is neither; the cast's exception, as 22018 of
/// a string that is no numeric literal.
fn double(item: &Item) -> Result<Item> {
    const NOT_NUMERIC: &str =
        "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C III 1 a] an SQL/JSON item of double() is not a number or a character string";
    let Some(scalar) = item.as_scalar() else { return Err(exception(SqlState::NON_NUMERIC_SQL_JSON_ITEM, NOT_NUMERIC)) };
    match scalar.value() {
        Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) => {
            let to = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
            cast(scalar.value().clone(), &to).map(super::item)
        }
        Value::Null
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Extension(_) => Err(exception(SqlState::NON_NUMERIC_SQL_JSON_ITEM, NOT_NUMERIC)),
    }
}

/// [§9.46 GR11g ii 6 C IV to VI] `compute` of each item of `base`, each a
/// number.
///
/// # Errors
///
/// SQLSTATE 22036, citing `rule`, if one is not; else `compute`'s error.
fn numeric(base: &[Item], compute: impl Fn(Number) -> Result<Value>, rule: &str) -> Result<Vec<Item>> {
    base.iter()
        .map(|each| {
            let n = number(each).ok_or_else(|| {
                exception(
                    SqlState::NON_NUMERIC_SQL_JSON_ITEM,
                    &format!("[ISO/IEC 9075-2:2023 §9.46 {rule}] an SQL/JSON item of an item method is not a number"),
                )
            })?;
            compute(n).map(item)
        })
        .collect()
}

#[cfg(test)]
mod tests;
