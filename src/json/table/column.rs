//! The value of a column of a `<JSON table primitive>` in the row of one
//! SQL/JSON item [ISO/IEC 9075-2:2023 §7.11 GR1e ii 1 to 3]: its ordinal,
//! or its path's SQL/JSON sequence of the item, with no PASSING clause,
//! cast to its type [§9.48] or serialized to it [§9.49].
//!
//! \[DESIGN\] A DEFAULT's `<value expression>` is evaluated only when its
//! behavior gives the column's value, so a DEFAULT that would raise an
//! error raises it only then.

use super::{Slot, TableColumn};
use crate::error::Result;
use crate::json::{Item, QueryOptions, ValueBehavior, Warned, cast_sequence, is_exception, serialize_sequence};
use crate::value::{DataType, Decimal, Value, cast_warned};

/// [§7.11 GR1e ii] The value of `column` in the row of `item`, the
/// `ordinal`-th of its primitive, and the warning a cast raised, if any;
/// `defaults` gives the value of the DEFAULT of a place.
///
/// # Errors
///
/// The exception condition its rules raise \[GR1e ii 2 D, GR1e ii 3 D\],
/// an error not one, or the error of a DEFAULT evaluated.
pub(super) fn value(
    column: &TableColumn,
    item: &Item,
    ordinal: usize,
    defaults: &mut impl FnMut(usize) -> Result<Value>,
) -> Result<Warned> {
    match column {
        // [GR1e ii 1] j.
        TableColumn::Ordinality => Ok((Value::Exact(Decimal::new(i128::try_from(ordinal).unwrap_or(i128::MAX), 0)), None)),
        // [GR1e ii 2 B, C] the path of the item, ALREADY PARSED, cast.
        TableColumn::Regular { path, to, on_empty, on_error } => regular(path.evaluate(item, &[]), to, (*on_empty, *on_error), defaults),
        // [GR1e ii 3 B, C II] the path of the item, serialized.
        TableColumn::Formatted(f) => {
            let options =
                QueryOptions { returning: &f.returning, wrapper: f.wrapper, quotes: f.quotes, on_empty: f.on_empty, on_error: f.on_error };
            serialize_sequence(f.path.evaluate(item, &[]), &options)
        }
    }
}

/// [§9.48] The sequence of `status` cast to `to`, ON EMPTY and ON ERROR
/// as the slots say, a DEFAULT's value asked of `defaults` only when it is
/// the one given.
fn regular(
    status: Result<Vec<Item>>,
    to: &DataType,
    (on_empty, on_error): (Slot, Slot),
    defaults: &mut impl FnMut(usize) -> Result<Value>,
) -> Result<Warned> {
    // [§9.48 GR4c] ON EMPTY is read only of an empty sequence.
    let empty = match on_empty {
        Slot::Error => ValueBehavior::Error,
        Slot::Null => ValueBehavior::Null,
        Slot::Default(place) => {
            if status.as_ref().is_ok_and(Vec::is_empty) {
                ValueBehavior::Default(defaults(place)?)
            } else {
                ValueBehavior::Null
            }
        }
    };
    let error = match on_error {
        Slot::Null => ValueBehavior::Null,
        Slot::Error | Slot::Default(_) => ValueBehavior::Error,
    };
    let cast = cast_sequence(status, &empty, &error, to);
    let Slot::Default(place) = on_error else { return cast };
    match cast {
        Ok(warned) => Ok(warned),
        // [§9.48 GR5d] the DEFAULT's value cast, in place of an exception
        // condition.
        Err(e) => {
            if is_exception(&e) {
                cast_warned(defaults(place)?, to)
            } else {
                Err(e)
            }
        }
    }
}
