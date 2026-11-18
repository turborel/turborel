//! The aggregate functions [XQueryFO §15.4]: `fn:avg`, `fn:max`,
//! `fn:min` and `fn:sum` (`fn:count` is with the functions of sequences),
//! each of the converted sequence: its untyped values cast to doubles, its
//! numbers promoted to one type [XQueryFO §6.2].

use std::cmp::Ordering;

use super::super::error::{ErrorCode, error};
use super::super::functions::Context;
use super::Builtin;
use super::args::collator;
use super::args::{arg, atomized, optional, optional_atomic};
use super::comparison::value_order;
use super::durations::{add_day_time_durations, add_year_month_durations, divide_day_time_duration, divide_year_month_duration};
use super::numeric::{Number, numeric_add, numeric_divide};
use crate::error::{Result, SqlError};
use crate::xml::{Atomic, AtomicType, Primitive, Sequence, cast};

/// The functions of [XQueryFO §15.4.2 to §15.4.5].
pub(super) const FUNCTIONS: [Builtin; 4] = [
    Builtin::function("avg", 1, 1, avg),
    Builtin::function("max", 1, 2, |f, a| extreme(f, a, Ordering::Greater)),
    Builtin::function("min", 1, 2, |f, a| extreme(f, a, Ordering::Less)),
    Builtin::function("sum", 1, 2, sum),
];

/// The error of values of no single type the function takes [XQueryFO
/// §15.4].
fn mixed(why: &str) -> SqlError {
    error(ErrorCode::FORG0006, &format!("{why} [XQueryFO §15.4]"))
}

/// The converted sequence of `value`: its atomic values, each untyped one
/// cast to a double.
fn converted(value: &Sequence) -> Result<Vec<Atomic>> {
    atomized(value)?.into_iter().map(|a| if a.kind == AtomicType::UntypedAtomic { cast(&a, AtomicType::Double) } else { Ok(a) }).collect()
}

/// What `values`, converted, are summed as: numbers, year-month or
/// day-time durations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Summed {
    Numbers,
    YearMonth,
    DayTime,
}

/// What `values` are summed as.
///
/// # Errors
///
/// err:FORG0006 unless all are numbers, all year-month durations or all
/// day-time durations.
fn summed(values: &[Atomic]) -> Result<Summed> {
    let of = |a: &Atomic| match a.kind.primitive() {
        Primitive::Decimal | Primitive::Integer | Primitive::Float | Primitive::Double => Some(Summed::Numbers),
        Primitive::YearMonthDuration => Some(Summed::YearMonth),
        Primitive::DayTimeDuration => Some(Summed::DayTime),
        Primitive::Untyped
        | Primitive::String
        | Primitive::Boolean
        | Primitive::Duration
        | Primitive::DateTime
        | Primitive::Time
        | Primitive::Date
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth
        | Primitive::HexBinary
        | Primitive::Base64Binary
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation => None,
    };
    let first = values.first().and_then(of).ok_or_else(|| mixed("the values are not numbers or durations"))?;
    if values.iter().all(|v| of(v) == Some(first)) { Ok(first) } else { Err(mixed("the values are not of one type")) }
}

/// The sum of `values`, none empty, as `kind`.
fn total(values: &[Atomic], kind: Summed) -> Result<Atomic> {
    let add = match kind {
        Summed::Numbers => numeric_add,
        Summed::YearMonth => add_year_month_durations,
        Summed::DayTime => add_day_time_durations,
    };
    let mut rest = values.iter();
    let first = rest.next().cloned().ok_or_else(|| mixed("no value"))?;
    let first = if kind == Summed::Numbers { Number::of(&first)?.atomic()? } else { first };
    rest.try_fold(first, |sum, v| add(&sum, v))
}

/// [XQueryFO §15.4.5] `fn:sum($arg as xdt:anyAtomicType*, $zero as
/// xdt:anyAtomicType?) as xdt:anyAtomicType?`: the sum of the converted
/// sequence; `$zero`, or the integer 0 if it is not given, of the empty
/// sequence.
///
/// # Errors
///
/// err:FORG0006 of values of no single type that sums.
fn sum(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let values = converted(arg(args, 0))?;
    if values.is_empty() {
        return match args.get(1) {
            Some(zero) => Ok(optional(optional_atomic(zero)?)),
            None => Ok(optional(Some(Atomic::new(AtomicType::Integer, "0")))),
        };
    }
    Ok(optional(Some(total(&values, summed(&values)?)?)))
}

/// [XQueryFO §15.4.2] `fn:avg($arg as xdt:anyAtomicType*) as
/// xdt:anyAtomicType?`: the sum of the converted sequence divided by its
/// count; the empty sequence of the empty sequence.
///
/// # Errors
///
/// As `sum`.
fn avg(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let values = converted(arg(args, 0))?;
    if values.is_empty() {
        return Ok(Sequence::empty());
    }
    let kind = summed(&values)?;
    let sum = total(&values, kind)?;
    let count = values.len().to_string();
    let average = match kind {
        Summed::Numbers => numeric_divide(&sum, &Atomic::new(AtomicType::Integer, count))?,
        Summed::YearMonth => divide_year_month_duration(&sum, &Atomic::new(AtomicType::Double, count))?,
        Summed::DayTime => divide_day_time_duration(&sum, &Atomic::new(AtomicType::Double, count))?,
    };
    Ok(optional(Some(average)))
}

/// The numeric type the numbers of `values` are promoted to, if all are
/// numbers.
fn promotion(values: &[Atomic]) -> Option<AtomicType> {
    let mut found = AtomicType::Integer;
    for v in values {
        let kind = match Number::of(v).ok()? {
            Number::Integer(_) => AtomicType::Integer,
            Number::Decimal(_) => AtomicType::Decimal,
            Number::Float(_) => AtomicType::Float,
            Number::Double(_) => AtomicType::Double,
        };
        let rank = |t| [AtomicType::Integer, AtomicType::Decimal, AtomicType::Float, AtomicType::Double].iter().position(|k| *k == t);
        if rank(kind) > rank(found) {
            found = kind;
        }
    }
    Some(found)
}

/// [XQueryFO §15.4.3, §15.4.4] `fn:max` if `wanted` is `Greater`, else
/// `fn:min`, `($arg as xdt:anyAtomicType*, $collation as string) as
/// xdt:anyAtomicType?`: the value of the converted sequence not less, or
/// not greater, than any other, the first of those; NaN if one is; the
/// empty sequence of the empty sequence.
///
/// # Errors
///
/// err:FORG0006 of values of no single ordered type; err:FOCH0002 of a
/// collation not supported.
fn extreme(focus: &Context<'_>, args: &[Sequence], wanted: Ordering) -> Result<Sequence> {
    let collation = collator(focus, args, 1)?;
    let values = converted(arg(args, 0))?;
    let Some(first) = values.first() else { return Ok(Sequence::empty()) };
    let promoted = promotion(&values);
    let not_a_number = values.iter().find(|v| super::numeric::is_nan(v));
    let mut best = match (not_a_number, promoted) {
        (Some(nan), Some(_)) => nan.clone(),
        (Some(_), None) => return Err(mixed("the values are not of one ordered type")),
        (None, _) => {
            let mut best = first.clone();
            for v in &values {
                let order = value_order(v, &best, &collation, i32::from(focus.timezone()))?;
                match order {
                    Some(o) => {
                        if o == wanted {
                            best = v.clone();
                        }
                    }
                    None => return Err(mixed("the values are not of one ordered type")),
                }
            }
            best
        }
    };
    if let Some(kind) = promoted {
        best = cast(&best, kind)?;
    }
    Ok(optional(Some(best)))
}

#[cfg(test)]
mod tests;
