//! \[EXTENSION\] Which calls of an SQL/JSON query operator may be part of
//! an index's key (RULES.md D92): those whose every row gives a
//! value and none an exception condition, the value the same whatever the
//! session.
//!
//! An exception condition the operator raises of a row [ISO/IEC
//! 9075-2:2023 §6.28 GR6, §6.35 GR6, §8.23 GR] is replaced as its ON ERROR
//! clause says, so a call is admitted only if that is not ERROR. Its
//! DEFAULT, an argument, is cast to the returned type only on a row that
//! takes it [§9.48 GR5d i], so it is a constant the cast takes, or of that
//! type already; one that may raise is no argument but evaluated in a
//! CASE (`functions/json/lazy.rs`), whose functions admit nothing.
//! JSON_QUERY raises 22001 of a text longer than its type whatever ON
//! ERROR says [§9.43 GR2a iii], so it returns JSON or the longest
//! character string. A datetime of a path's `datetime()`, `time_tz()` or
//! `timestamp_tz()`, or of a type with a time zone, is of the session's
//! time zone, so neither is admitted; nor is a large object, which a call
//! reads whole only up to a chunk.

use super::{JsonCall, JsonOperation, ValueSlot};
use crate::index::expression::KeyArgument;
use crate::json::{ExistsBehavior, IMPLICIT_RETURNING, QueryBehavior};
use crate::value::{DataType, cast_warned};

impl JsonCall {
    /// \[EXTENSION\] Whether a call of `args`, its context item's, its
    /// PASSING arguments' and its DEFAULTs', may be part of an index's key
    /// (`ScalarFunction::index_safe`, RULES.md D92).
    pub fn key_safe(&self, args: &[KeyArgument<'_>]) -> bool {
        let Some((context, rest)) = args.split_first() else { return false };
        let Some((passing, defaults)) = rest.split_at_checked(self.passing.len()) else { return false };
        let item = context.data_type.is_some_and(|t| *t == DataType::Json || plain(t));
        item && passing.iter().all(|p| p.data_type.is_some_and(scalar)) && !self.path.zoned() && self.operation_safe(defaults)
    }

    /// Whether its operation raises no exception condition, its DEFAULTs
    /// `defaults`.
    fn operation_safe(&self, defaults: &[KeyArgument<'_>]) -> bool {
        match &self.operation {
            JsonOperation::Exists(on_error) => *on_error != ExistsBehavior::Error,
            JsonOperation::Value { returning, on_empty, on_error } => {
                let slots = [*on_empty, *on_error];
                let taken = slots.iter().filter(|s| **s == ValueSlot::Default).count() == defaults.len();
                *on_error != ValueSlot::Error && taken && !zoned(returning) && defaults.iter().all(|d| cast_to(d, returning))
            }
            JsonOperation::Query { returning, on_error, .. } => {
                *on_error != QueryBehavior::Error && (*returning == DataType::Json || *returning == IMPLICIT_RETURNING)
            }
        }
    }
}

/// A character string type of no large object.
fn plain(t: &DataType) -> bool {
    t.is_character() && !t.is_lob()
}

/// Whether a PASSING argument of type `t` is one an SQL/JSON scalar takes
/// whole and of no time zone: a character string, a number, a truth value,
/// a datetime without time zone, or JSON.
fn scalar(t: &DataType) -> bool {
    let other = matches!(t, DataType::Boolean | DataType::Date | DataType::Json) || t.is_numeric();
    plain(t) || other || (t.is_datetime() && !zoned(t))
}

/// Whether `t` is a datetime type with a time zone.
fn zoned(t: &DataType) -> bool {
    matches!(t, DataType::Time { zoned: true, .. } | DataType::Timestamp { zoned: true, .. })
}

/// Whether the DEFAULT `d` is cast to `returning` without an error of any
/// row: a constant whose cast succeeds, or a value of that type.
fn cast_to(d: &KeyArgument<'_>, returning: &DataType) -> bool {
    match d.constant {
        Some(v) => cast_warned(v.clone(), returning).is_ok(),
        None => d.data_type == Some(returning),
    }
}
