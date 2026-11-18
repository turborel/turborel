//! The general functions and operators on sequences [XQueryFO §15.1] and
//! those that test their cardinality [XQueryFO §15.2], `fn:count`
//! [XQueryFO §15.4.1], and `op:to` [XQueryFO §15.5.1].
//!
//! \[DESIGN\] RULES.md D86: `fn:distinct-values` keeps the first of each
//! set of equal values, in the order they come, and `fn:unordered` gives
//! its items in the order they came, each an order [XQueryFO §15.1.6,
//! §15.1.11] leaves to the implementation.

use super::super::error::{ErrorCode, error};
use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, atomized, boolean, collator, integer, integer_value, items, optional, within_items};
use super::comparison::value_equal;
use super::decimal::Decimal;
use super::rounding::round_real;
use crate::error::Result;
use crate::xml::{Atomic, Item, Sequence};

/// The functions of [XQueryFO §15.1, §15.2] and `fn:count`.
pub(super) const FUNCTIONS: [Builtin; 13] = [
    Builtin::function("index-of", 2, 3, index_of),
    Builtin::function("empty", 1, 1, |_, a| Ok(boolean(arg(a, 0).items().is_empty()))),
    Builtin::function("exists", 1, 1, |_, a| Ok(boolean(!arg(a, 0).items().is_empty()))),
    Builtin::function("distinct-values", 1, 2, distinct_values),
    Builtin::function("insert-before", 3, 3, insert_before),
    Builtin::function("remove", 2, 2, remove),
    Builtin::function("reverse", 1, 1, reverse),
    Builtin::function("subsequence", 2, 3, subsequence),
    Builtin::function("unordered", 1, 1, |_, a| Ok(arg(a, 0).clone())),
    Builtin::function("zero-or-one", 1, 1, zero_or_one),
    Builtin::function("one-or-more", 1, 1, one_or_more),
    Builtin::function("exactly-one", 1, 1, exactly_one),
    Builtin::function("count", 1, 1, |_, a| Ok(count(arg(a, 0)))),
];

/// [XQueryFO §15.1.2] `op:concatenate`: the items of `a`, then those of
/// `b`.
///
/// # Errors
///
/// SQLCODE -900 if they are more than `MAX_ITEMS`.
pub fn concatenate(a: &Sequence, b: &Sequence) -> Result<Sequence> {
    items(a.items().iter().chain(b.items()).cloned().collect())
}

/// [XQueryFO §15.1.3] `fn:index-of($seqParam as xdt:anyAtomicType*,
/// $srchParam as xdt:anyAtomicType, $collation as xs:string) as
/// xs:integer*`: the positions, from 1, of the values `eq` to the one
/// sought; values not comparable with it distinct.
fn index_of(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let collation = collator(focus, args, 2)?;
    let sought = super::args::atomic(arg(args, 1))?;
    let mut found = Vec::new();
    for (at, value) in atomized(arg(args, 0))?.iter().enumerate() {
        if value_equal(value, &sought, &collation, i32::from(focus.timezone()))? == Some(true) {
            let position = i128::try_from(at.saturating_add(1)).unwrap_or(i128::MAX);
            found.push(Item::Atomic(integer_value(&Decimal::integer(position))));
        }
    }
    items(found)
}

/// [XQueryFO §15.1.6] `fn:distinct-values($arg as xdt:anyAtomicType*,
/// $collation as xs:string) as xdt:anyAtomicType*`: the values with all
/// but the first of each set `eq` to one another removed, NaN once.
///
/// # Errors
///
/// err:FOCH0002 of a collation not supported; the statement's
/// cancellation.
fn distinct_values(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let collation = collator(focus, args, 1)?;
    let mut kept: Vec<Atomic> = Vec::new();
    for value in atomized(arg(args, 0))? {
        // [DESIGN] RULES.md D44: each value is compared with those kept,
        // so the statement's cancellation is looked at as it goes.
        focus.check()?;
        let mut seen = false;
        for k in &kept {
            if value_equal(k, &value, &collation, i32::from(focus.timezone()))? == Some(true)
                || (super::numeric::is_nan(k) && super::numeric::is_nan(&value))
            {
                seen = true;
                break;
            }
        }
        if !seen {
            kept.push(value);
        }
    }
    items(kept.into_iter().map(Item::Atomic).collect())
}

/// The position `args`'s argument `n` gives, an integer saturated within
/// an `i128`.
fn position(args: &[Sequence], n: usize) -> Result<i128> {
    let p = integer(arg(args, n))?;
    Ok(p.to_i128().unwrap_or(if p.is_negative() { i128::MIN } else { i128::MAX }))
}

/// [XQueryFO §15.1.7] `fn:insert-before($target as item()*, $position as
/// xs:integer, $inserts as item()*) as item()*`: the inserts put before
/// the item at the position, at the start below 1, at the end past it.
fn insert_before(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let target = arg(args, 0).items();
    let inserts = arg(args, 2).items();
    let place = usize::try_from(position(args, 1)?.saturating_sub(1).max(0)).unwrap_or(usize::MAX).min(target.len());
    within_items(target.len().saturating_add(inserts.len()))?;
    let (before, after) = target.split_at_checked(place).unwrap_or((target, &[]));
    items(before.iter().chain(inserts).chain(after).cloned().collect())
}

/// [XQueryFO §15.1.8] `fn:remove($target as item()*, $position as
/// xs:integer) as item()*`: the item at the position removed, if there is
/// one there.
fn remove(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let at = position(args, 1)?;
    let target = arg(args, 0).items();
    let kept = target.iter().enumerate().filter(|(i, _)| i128::try_from(*i).ok().and_then(|i| i.checked_add(1)) != Some(at));
    items(kept.map(|(_, item)| item.clone()).collect())
}

/// [XQueryFO §15.1.9] `fn:reverse($arg as item()*) as item()*`.
fn reverse(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    items(arg(args, 0).items().iter().rev().cloned().collect())
}

/// [XQueryFO §15.1.10] `fn:subsequence($sourceSeq as item()*,
/// $startingLoc as xs:double, $length as xs:double) as item()*`: the
/// items at positions `p` such that `round(start) <= p < round(start) +
/// round(length)`, the length infinite if not given.
fn subsequence(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let start = round_real(super::args::double(arg(args, 1))?);
    let end = match args.get(2) {
        Some(length) => start + round_real(super::args::double(length)?),
        None => f64::INFINITY,
    };
    let mut position = 0.0_f64;
    let kept = arg(args, 0).items().iter().filter(|_| {
        position += 1.0_f64;
        position >= start && position < end
    });
    items(kept.cloned().collect())
}

/// [XQueryFO §15.2.1] `fn:zero-or-one($arg as item()*) as item()?`.
///
/// # Errors
///
/// err:FORG0003 of more than one item.
fn zero_or_one(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let value = arg(args, 0);
    if value.items().len() > 1 {
        return Err(error(ErrorCode::FORG0003, "fn:zero-or-one is given more than one item [XQueryFO §15.2.1]"));
    }
    Ok(value.clone())
}

/// [XQueryFO §15.2.2] `fn:one-or-more($arg as item()*) as item()+`.
///
/// # Errors
///
/// err:FORG0004 of the empty sequence.
fn one_or_more(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let value = arg(args, 0);
    if value.items().is_empty() {
        return Err(error(ErrorCode::FORG0004, "fn:one-or-more is given no item [XQueryFO §15.2.2]"));
    }
    Ok(value.clone())
}

/// [XQueryFO §15.2.3] `fn:exactly-one($arg as item()*) as item()`.
///
/// # Errors
///
/// err:FORG0005 of no item, or more than one.
fn exactly_one(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let value = arg(args, 0);
    if value.items().len() != 1 {
        return Err(error(ErrorCode::FORG0005, &format!("fn:exactly-one is given {} items [XQueryFO §15.2.3]", value.items().len())));
    }
    Ok(value.clone())
}

/// [XQueryFO §15.4.1] `fn:count($arg as item()*) as xs:integer`.
fn count(value: &Sequence) -> Sequence {
    let n = i128::try_from(value.items().len()).unwrap_or(i128::MAX);
    optional(Some(integer_value(&Decimal::integer(n))))
}

/// [XQueryFO §15.5.1] `op:to`: the integers from `first` to `last`, in
/// order; the empty sequence if `first` is greater.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no integer; SQLCODE -900 if they
/// are more than `MAX_ITEMS`.
pub fn to(first: &Atomic, last: &Atomic) -> Result<Sequence> {
    let (a, b) = (super::args::integer_of(first)?, super::args::integer_of(last)?);
    let span = b.subtract(&a).add(&Decimal::integer(1));
    if span.is_negative() || span.is_zero() {
        return Ok(Sequence::empty());
    }
    let count = span.to_i128().and_then(|n| usize::try_from(n).ok()).unwrap_or(usize::MAX);
    within_items(count)?;
    let mut out = Vec::with_capacity(count);
    let mut n = a;
    for _ in 0..count {
        out.push(Item::Atomic(integer_value(&n)));
        n = n.add(&Decimal::integer(1));
    }
    Ok(Sequence::new(out))
}

#[cfg(test)]
mod tests;
