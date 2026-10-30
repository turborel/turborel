//! \[DESIGN\] RULES.md D31: the built-in set functions of approximate
//! numbers given as `f64`s (`storage::Floats`), a vector at a time, no
//! value made of each: how many are not null, their sum, and the greatest
//! or least, each of the rows given or all, nulls left out [X3.135-1986
//! §5.8 GR1], in the order a row at a time takes them, so each total is
//! the same. Split from functions.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;

use crate::storage::Floats;
use crate::value::Value;

/// The numbers of `floats` of the rows `rows`, or all, not null.
fn numbers<'f>(floats: &'f Floats, rows: Option<&'f [usize]>) -> Box<dyn Iterator<Item = f64> + 'f> {
    match rows {
        Some(rows) => Box::new(rows.iter().filter_map(move |&n| floats.get(n))),
        None if floats.nulls.is_none() => Box::new(floats.numbers.iter().copied()),
        None => Box::new((0..floats.numbers.len()).filter_map(move |n| floats.get(n))),
    }
}

/// How many of the rows `rows` of `floats`, or all, are not null.
pub(super) fn count(floats: &Floats, rows: Option<&[usize]>) -> usize {
    numbers(floats, rows).count()
}

/// `start`, if any, plus each of the rows `rows` of `floats`, or all, not
/// null, in order, and whether there is a sum, of `start` or of one: none
/// if a running sum is not finite, which `arith` refuses [X3.135-1986 §5.9
/// SR5].
pub(super) fn sum_from(start: Option<f64>, floats: &Floats, rows: Option<&[usize]>) -> Option<(f64, bool)> {
    let mut sum = start;
    for f in numbers(floats, rows) {
        let next = sum.map_or(f, |s| s + f);
        if !next.is_finite() {
            return None;
        }
        sum = Some(next);
    }
    Some((sum.unwrap_or(0.0_f64), sum.is_some()))
}

/// The greatest, if `max`, or least of the rows `rows` of `floats`, or
/// all, not null: the first of those equal [X3.135-1986 §5.8 GR4d].
pub(super) fn extreme(floats: &Floats, rows: Option<&[usize]>, max: bool) -> Option<Value> {
    let wanted = if max { Ordering::Greater } else { Ordering::Less };
    numbers(floats, rows).reduce(|best, f| if f.partial_cmp(&best) == Some(wanted) { f } else { best }).map(Value::Approx)
}
