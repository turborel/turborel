//! \[DESIGN\] Calls of scalar functions over vectors (RULES.md D363): a
//! function with a body over vectors (`ScalarFunction::eval_batch`), as
//! ABS, CHAR_LENGTH, UPPER, LOWER and EXTRACT have, is called once for
//! the rows given; any other is called for each row's values, borrowed
//! from its arguments' vectors (`call_borrowed`), as a row's evaluation
//! calls it, the tree above it still evaluated a vector at a time. A
//! function that may raise a warning is called as a row's evaluation
//! calls it (`call_warned`), and a row that raises one gives none, as one
//! that raises an error does, so the batch is evaluated a row at a time,
//! its warnings in order. A sort key under a collation is made of the
//! values of a vector together (`CollationRef::sort_keys`), but under one
//! whose key is the text itself, where each is the text's octets, taken
//! from a vector of texts in place (RULES.md D1183).

use super::vector::{Vector, values};
use crate::exec::Subqueries;
use crate::functions::{Arg, ScalarRef};
use crate::storage::Column;
use crate::value::collation::CollationRef;
use crate::value::{Binary, Value};

/// `v` as a function's argument over vectors: its column, or its
/// constant.
fn arg_of<'v>(v: &'v Vector<'_>) -> Arg<'v> {
    match v {
        Vector::Column(c) => Arg::Column(c),
        Vector::Owned(c) => Arg::Column(c),
        Vector::Const(value) => Arg::Const(value),
    }
}

/// `function` of `args`, `count` rows of each, in `statement`: none if a
/// row's call would raise an error or a warning.
pub(super) fn called(function: &ScalarRef, args: &[Vector<'_>], (count, statement): (usize, &dyn Subqueries)) -> Option<Vector<'static>> {
    let f = &function.0;
    if !f.warns() {
        let given: Vec<Arg<'_>> = args.iter().map(arg_of).collect();
        if let Some(made) = f.eval_batch(&given, count) {
            return made.ok().map(Vector::Owned);
        }
    }
    let seqs: Vec<_> = args.iter().map(Vector::seq).collect();
    let temp = statement.temp_lobs();
    let mut made = Vec::with_capacity(count);
    let mut row: Vec<&Value> = Vec::with_capacity(args.len());
    for n in 0..count {
        row.clear();
        row.extend(seqs.iter().map(|s| s.get(n)));
        let value = if f.warns() {
            let owned: Vec<Value> = row.iter().map(|v| (*v).clone()).collect();
            match f.call_warned(&owned, temp) {
                Ok((v, None)) => v,
                Ok((_, Some(_))) | Err(_) => return None,
            }
        } else {
            f.call_borrowed(&row, temp).ok()?
        };
        made.push(value);
    }
    Some(values(made))
}

/// The sort keys of each of `count` rows of `x` under `collation`, made
/// together: none if making them would raise.
pub(super) fn sort_keys(x: &Vector<'_>, collation: &CollationRef, count: usize) -> Option<Vector<'static>> {
    if collation.is_octets() {
        return Some(values(octets(x, count)));
    }
    let seq = x.seq();
    let given: Vec<Value> = (0..count).map(|n| seq.get(n).clone()).collect();
    collation.sort_keys(&given).ok().map(values)
}

/// [ISO/IEC 9075-2:2023 §4.3.2] (RULES.md D1183) The sort keys of each of
/// `count` rows of `x` under a collation whose key is the text itself
/// (`CollationRef::is_octets`): each string's octets as a binary string,
/// read from a vector of texts with no value made of the text first; a
/// null stays null, as `CollationRef::sort_key` leaves it.
fn octets(x: &Vector<'_>, count: usize) -> Vec<Value> {
    let keyed = |v: &Value| if let Value::Char(s) = v { Value::Binary(Binary::of(s.as_bytes())) } else { v.clone() };
    if let Some(Column::Texts(texts)) = x.column() {
        return (0..count).map(|n| texts.get(n).map_or(Value::Null, |t| Value::Binary(Binary::of(t.as_bytes())))).collect();
    }
    let seq = x.seq();
    (0..count).map(|n| keyed(seq.get(n))).collect()
}
