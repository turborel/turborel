//! JSON_VALUE [ISO/IEC 9075-2:2023 §6.28], `<JSON value function>`: an
//! SQL value of a predefined type taken of JSON text; and an SQL/JSON
//! sequence cast to an SQL type [§9.48], as it is cast.

use super::{CommonSyntax, Warned, caught, exception, process};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::json::Item;
use crate::value::datetime::Moment;
use crate::value::{DataType, Value, cast_warned};

/// [§6.28] A `<JSON value empty behavior>` or `<JSON value error
/// behavior>`: what JSON_VALUE gives of an empty SQL/JSON sequence, or in
/// place of an exception condition. \[SR4, SR5\] Without one, NULL is
/// implicit, `ValueBehavior::Null`, ON EMPTY and ON ERROR alike.
#[derive(Debug, Clone)]
pub enum ValueBehavior {
    /// `ERROR`: the exception condition, of no SQL/JSON item if empty.
    Error,
    /// `NULL`: the null value; the default \[SR4, SR5\].
    Null,
    /// `DEFAULT <value expression>`: the value of the expression, here
    /// evaluated, cast to the returned type [§9.48 GR4c iii, GR5d].
    Default(Value),
}

/// [§6.13 SR7] What a value of an SQL/JSON scalar's type may be cast to:
/// the kinds of type JSON_VALUE returns \[§6.28 SR2\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// A character string type: C.
    Character,
    /// A numeric type: EN, AN, DF.
    Numeric,
    /// BOOLEAN: BO.
    Boolean,
    /// DATE: D.
    Date,
    /// TIME: T.
    Time,
    /// TIMESTAMP: TS.
    Timestamp,
}

// [§6.13 SR7] The table's rows of the SQL/JSON scalars' types: the kinds
// of target type each is cast to.
/// C: to all.
const FROM_CHARACTER: [Target; 6] = [Target::Character, Target::Numeric, Target::Boolean, Target::Date, Target::Time, Target::Timestamp];
/// EN, AN, DF: to C, EN, AN and DF.
const FROM_NUMBER: [Target; 2] = [Target::Character, Target::Numeric];
/// BO: to C and BO.
const FROM_BOOLEAN: [Target; 2] = [Target::Character, Target::Boolean];
/// D: to C, D and TS.
const FROM_DATE: [Target; 3] = [Target::Character, Target::Date, Target::Timestamp];
/// T: to C, T and TS.
const FROM_TIME: [Target; 3] = [Target::Character, Target::Time, Target::Timestamp];
/// TS: to C, D, T and TS.
const FROM_TIMESTAMP: [Target; 4] = [Target::Character, Target::Date, Target::Time, Target::Timestamp];

/// [§6.28 SR2] The kind of `to` if it is a type JSON_VALUE returns: a
/// predefined character string, numeric, Boolean or datetime type.
fn target(to: &DataType) -> Option<Target> {
    match to {
        DataType::Character { .. } | DataType::Varying { .. } => Some(Target::Character),
        DataType::Lob { .. } => to.is_character().then_some(Target::Character),
        DataType::Exact { .. } | DataType::Approximate { .. } | DataType::DecFloat { .. } => Some(Target::Numeric),
        DataType::Boolean => Some(Target::Boolean),
        DataType::Date => Some(Target::Date),
        DataType::Time { .. } => Some(Target::Time),
        DataType::Timestamp { .. } => Some(Target::Timestamp),
        DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Json
        | DataType::Xml(_)
        | DataType::Extension(_) => None,
    }
}

/// [§6.13 SR7] The kinds of type a value of `value`'s type may be cast
/// to; none if it is of no SQL/JSON scalar's type.
fn targets(value: &Value) -> &'static [Target] {
    match value {
        Value::Char(_) => &FROM_CHARACTER,
        Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) => &FROM_NUMBER,
        Value::Bool(_) => &FROM_BOOLEAN,
        Value::Datetime(d) => match d.moment {
            Moment::Date => &FROM_DATE,
            Moment::Time => &FROM_TIME,
            Moment::Timestamp => &FROM_TIMESTAMP,
        },
        Value::Null
        | Value::Binary(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => &[],
    }
}

/// [§6.28 GR] JSON_VALUE of the `<JSON API common syntax>` `common`, its
/// `<JSON returning clause>`'s `<data type>` `returning` (without one,
/// `query::IMPLICIT_RETURNING` \[SR1\]), and its ON EMPTY and ON ERROR
/// behaviours: the null value if the context item is \[GR2\]; else the
/// SQL/JSON sequence of the common syntax \[GR3\] cast to `returning`
/// (`cast_sequence`) \[GR5\], and the warning the cast raised, if any.
///
/// # Errors
///
/// SQLCODE -103 if `returning` is no predefined character string,
/// numeric, Boolean or datetime type \[SR2\]; else as `process` and
/// `cast_sequence` \[GR6\].
pub fn json_value(common: &CommonSyntax<'_>, returning: &DataType, on_empty: &ValueBehavior, on_error: &ValueBehavior) -> Result<Warned> {
    let Some(kind) = target(returning) else {
        return Err(SqlError::new(
            Code::SyntaxRule,
            format!(
                "[ISO/IEC 9075-2:2023 §6.28 SR2] JSON_VALUE returns a character string, numeric, Boolean or datetime type, not {returning}"
            ),
        ));
    };
    let Some(status) = process(common).transpose() else { return Ok((Value::Null, None)) };
    cast_to(status, on_empty, on_error, (returning, kind))
}

/// [§9.48 GR4c iii, GR5d] Which result JSON_VALUE takes: the value its
/// rules give, or the DEFAULT of ON EMPTY or of ON ERROR, whose value is
/// evaluated only then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// The value `json_value` gives, a behavior's NULL among them.
    Value,
    /// DEFAULT ON EMPTY: the sequence is empty \[GR4c iii\].
    EmptyDefault,
    /// DEFAULT ON ERROR: an exception condition was raised \[GR5d\].
    ErrorDefault,
}

/// [§6.28 GR] Which result JSON_VALUE takes of `common`, `returning`,
/// `on_empty` and `on_error`, as `json_value` takes them, without taking
/// the DEFAULT of either: a `ValueBehavior::Default`'s value is not read.
/// So the SQL layer evaluates a DEFAULT's `<value expression>` only when
/// its behavior is taken, as §9.48 evaluates it \[GR4c iii, GR5d\].
///
/// # Errors
///
/// As `json_value`, of a behavior that is not DEFAULT.
pub fn json_value_taken(
    common: &CommonSyntax<'_>,
    returning: &DataType,
    on_empty: &ValueBehavior,
    on_error: &ValueBehavior,
) -> Result<Taken> {
    let Some(kind) = target(returning) else { return json_value(common, returning, on_empty, on_error).map(|_| Taken::Value) };
    let Some(status) = process(common).transpose() else { return Ok(Taken::Value) };
    let temp = status.and_then(|seq| {
        if seq.is_empty() && matches!(on_empty, ValueBehavior::Default(_)) {
            return Ok(Taken::EmptyDefault);
        }
        cast_items(seq, on_empty, (returning, kind)).map(|_| Taken::Value)
    });
    caught(temp, |error| match on_error {
        ValueBehavior::Error => Err(error),
        ValueBehavior::Null => Ok(Taken::Value),
        ValueBehavior::Default(_) => Ok(Taken::ErrorDefault),
    })
}

/// [§9.48] The SQL value of type `to` the SQL/JSON sequence of `status`,
/// STATUS IN and SQL/JSON SEQUENCE, casts to, and the warning the cast
/// raised, if any: its one item cast \[GR4b\], of none as `on_empty` says
/// \[GR4c\]. An exception condition, of `status`, the rules or a cast, is
/// replaced as `on_error` says \[GR5\]; an error not one is raised
/// (`query::is_exception`).
///
/// # Errors
///
/// SQLCODE -103 if `to` is of none of the kinds JSON_VALUE returns
/// [§6.28 SR2]; an error not an exception condition; if `on_error` is
/// ERROR \[GR5b\], the exception condition: of `status`; SQLSTATE 22034 if
/// the sequence has more than one item \[GR4a\], 2203F if its item is an
/// array or an object \[GR4b i\], 2203G if its item's type cannot be cast
/// to `to` \[GR4b iii 1\], 22035 if it is empty and `on_empty` is ERROR
/// \[GR4c i\], or that of a cast \[GR4b iii 2, GR4c iii\]; if `on_error` is
/// DEFAULT, that of its value's cast \[GR5d i\].
pub fn cast_sequence(status: Result<Vec<Item>>, on_empty: &ValueBehavior, on_error: &ValueBehavior, to: &DataType) -> Result<Warned> {
    let Some(kind) = target(to) else {
        return Err(SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §6.28 SR2] an SQL/JSON sequence is not cast to {to}")));
    };
    cast_to(status, on_empty, on_error, (to, kind))
}

/// `cast_sequence` to `to`, of the kind it is.
fn cast_to(status: Result<Vec<Item>>, on_empty: &ValueBehavior, on_error: &ValueBehavior, to: (&DataType, Target)) -> Result<Warned> {
    let temp = status.and_then(|seq| cast_items(seq, on_empty, to));
    caught(temp, |error| match on_error {
        ValueBehavior::Error => Err(error),
        ValueBehavior::Null => Ok((Value::Null, None)),
        ValueBehavior::Default(value) => cast_warned(value.clone(), to.0),
    })
}

/// [§9.48 GR4] The value `seq` casts to.
fn cast_items(seq: Vec<Item>, on_empty: &ValueBehavior, (to, kind): (&DataType, Target)) -> Result<Warned> {
    let mut items = seq.into_iter();
    let Some(item) = items.next() else {
        return match on_empty {
            ValueBehavior::Error => Err(exception(
                SqlState::NO_SQL_JSON_ITEM,
                "[ISO/IEC 9075-2:2023 §9.48 GR4c i] the SQL/JSON path expression gives no SQL/JSON item, and ERROR ON EMPTY",
            )),
            ValueBehavior::Null => Ok((Value::Null, None)),
            ValueBehavior::Default(value) => cast_warned(value.clone(), to),
        };
    };
    if items.next().is_some() {
        return Err(exception(
            SqlState::MORE_THAN_ONE_SQL_JSON_ITEM,
            "[ISO/IEC 9075-2:2023 §9.48 GR4a] the SQL/JSON path expression gives more than one SQL/JSON item",
        ));
    }
    let scalar = match item {
        Item::Null => return Ok((Value::Null, None)),
        Item::Scalar(scalar) => scalar,
        Item::Array(_) | Item::Object(_) => {
            return Err(exception(
                SqlState::SQL_JSON_SCALAR_REQUIRED,
                "[ISO/IEC 9075-2:2023 §9.48 GR4b i] the SQL/JSON item is an array or an object, not a scalar",
            ));
        }
    };
    if !targets(scalar.value()).contains(&kind) {
        return Err(exception(
            SqlState::SQL_JSON_ITEM_CANNOT_BE_CAST_TO_TARGET_TYPE,
            &format!("[ISO/IEC 9075-2:2023 §9.48 GR4b iii 1] the SQL/JSON item {} cannot be cast to {to}", scalar.value()),
        ));
    }
    cast_warned(scalar.value().clone(), to)
}

#[cfg(test)]
mod tests;
