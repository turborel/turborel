//! \[DESIGN\] RULES.md D31: a conjunct judged over a vector of approximate
//! numbers (`storage::Floats`), no value made of each: each compared with
//! its constants as two numbers compare by algebraic value [X3.135-1986
//! §5.11 GR4], an exact constant as exactly as a row's judgment compares
//! it (`value::compare`); a null true of no comparison [§5.11 GR2], of IS
//! NULL only [§5.15]. Split from judge.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;

use super::Judged;
use super::ints::holds;
use crate::storage::Floats;
use crate::value::{Value, compare};

/// How `f` compares with `constant`: as `f64`s of an approximate one.
fn ordering(f: f64, constant: &Value) -> Option<Ordering> {
    match constant {
        Value::Approx(c) => f.partial_cmp(c),
        Value::Null | Value::Char(_) | Value::Exact(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_) => {
            compare(&Value::Approx(f), constant)
        }
    }
}

/// The rows of `kept` that `judged` keeps of `floats`, if it is judged of
/// approximate numbers.
pub(super) fn kept(judged: &Judged, floats: &Floats, kept: &[usize]) -> Option<Vec<usize>> {
    let keep = |truth: &dyn Fn(f64) -> bool| Some(kept.iter().copied().filter(|&n| floats.get(n).is_some_and(truth)).collect());
    match judged {
        // [X3.135-1986 §5.15 GR2-GR4] never unknown.
        Judged::IsNull(_, not) => Some(kept.iter().copied().filter(|&n| floats.is_null(n) != *not).collect()),
        Judged::Compare(_, op, constant) => keep(&|f| holds(*op, ordering(f, constant))),
        // [X3.135-1986 §5.13 GR2, §5.18] an OR of equalities: true if one
        // is; NOT of it true only if every one is false.
        Judged::In(_, list, false) => keep(&|f| list.iter().any(|c| ordering(f, c) == Some(Ordering::Equal))),
        Judged::In(_, list, true) => keep(&|f| list.iter().all(|c| ordering(f, c).is_some_and(Ordering::is_ne))),
        Judged::Like(..) | Judged::Compared(..) => None,
    }
}
