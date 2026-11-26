//! \[DESIGN\] Predicates over vectors, each giving truth values a bit a
//! row, Unknown null (RULES.md D362). A comparison [X3.135-1986 §5.11] of
//! exact numbers compares their integers at the greater scale, as
//! `Decimal::cmp_value` does [GR4]; of approximate numbers by
//! `f64::partial_cmp`, NaN unknown, as `value::compare` does; of
//! character strings as `compare_strings` pads them; of datetimes of one
//! type by their instants, of two unknown [ISO/IEC 9075:1992 §8.2 GR5];
//! of truth values False below True; and of anything else each row's
//! values as a row's evaluation compares them (`compared_read`). A null
//! makes a comparison unknown [GR2]. IS DISTINCT FROM, LIKE of a constant
//! pattern and IN a list apply their rows' rules to each row's values.

use std::cmp::Ordering;

use super::ints::{Exact, exact};
use super::vector::{Marks, Vector, either, marks, some};
use crate::exec::Subqueries;
use crate::exec::eval::{compared_read, like_truth};
use crate::exec::{Escape, like_escape};
use crate::kernels::Kernels;
use crate::logical::{InSet, Truth};
use crate::storage::mask::{WORD_BITS, index};
use crate::storage::{Bools, Column, Mask, Texts};
use crate::value::comparison::compare_strings;
use crate::value::{CompOp, LikePattern, Value, distinct as value_distinct, pow10};

/// Truth values of `count` rows, of which `nulls` are unknown, and each
/// other's as `f` judges it: true, false, or none for unknown.
fn judged(count: usize, nulls: Option<Mask>, f: impl Fn(usize) -> Option<bool>) -> Bools {
    let words = count.div_ceil(WORD_BITS);
    let (mut bits, mut unknown) = (Vec::with_capacity(words), Vec::with_capacity(words));
    for w in 0..words {
        let (mut word, mut lost) = (0_u64, 0_u64);
        let first = w.saturating_mul(WORD_BITS);
        for b in 0..WORD_BITS.min(count.saturating_sub(first)) {
            match f(first.saturating_add(b)) {
                Some(t) => word |= u64::from(t) << b,
                None => lost |= 1_u64 << b,
            }
        }
        bits.push(word);
        unknown.push(lost);
    }
    let unknown = Mask::from_words(unknown, count);
    let nulls = match (nulls, unknown.any()) {
        (Some(n), true) => some(n.or(&unknown)),
        (Some(n), false) => some(n),
        (None, true) => Some(unknown),
        (None, false) => None,
    };
    let bits = Mask::from_words(bits, count);
    let bits = nulls.as_ref().map_or_else(|| bits.clone(), |n| bits.and_not(n));
    Bools::new(bits, nulls)
}

/// The nulls of `v`'s column, if it marks them and any is.
fn nulls_of<'v>(v: &'v Vector<'_>) -> Option<&'v Mask> {
    match v.column().map(marks) {
        Some(Marks::Of(nulls)) => nulls,
        Some(Marks::Values) | None => None,
    }
}

/// "`x op y`" of each of `count` rows: none if a row's would raise.
pub(super) fn compared(op: CompOp, x: &Vector<'_>, y: &Vector<'_>, count: usize) -> Option<Vector<'static>> {
    if x.is_null_const() || y.is_null_const() {
        return Some(Vector::Const(Value::Null));
    }
    let nulls = || either(nulls_of(x), nulls_of(y));
    if let (Some(a), Some(b)) = (exact(x), exact(y))
        && let Some(made) = exact_compared(op, (&a, &b), count, nulls())
    {
        return Some(bools(made));
    }
    if let Some(made) = typed(op, (x, y), count, nulls()) {
        return Some(bools(made));
    }
    let (a, b) = (x.seq(), y.seq());
    let mut truths = Vec::with_capacity(count);
    for n in 0..count {
        truths.push(compared_read(a.get(n), op, b.get(n)).ok()?);
    }
    Some(bools(judged(count, None, |n| decided(truths.get(n).copied().unwrap_or(Truth::Unknown)))))
}

/// A vector of truth values made.
fn bools(made: Bools) -> Vector<'static> {
    Vector::Owned(Column::Bools(made))
}

/// The truth `t` is, none for Unknown.
fn decided(t: Truth) -> Option<bool> {
    match t {
        Truth::True => Some(true),
        Truth::False => Some(false),
        Truth::Unknown => None,
    }
}

/// [X3.135-1986 §5.11 GR4] "`x op y`" of exact numbers as integers, at
/// the greater scale in 128 bits: none if a scale's power does not fit.
fn exact_compared(op: CompOp, (x, y): (&Exact<'_>, &Exact<'_>), count: usize, nulls: Option<Mask>) -> Option<Bools> {
    let (sx, sy) = (scale_of(x), scale_of(y));
    let scale = sx.max(sy);
    let (fx, fy) = (pow10(scale.checked_sub(sx)?)?, pow10(scale.checked_sub(sy)?)?);
    let unit = |e: &Exact<'_>, n: usize| match e {
        Exact::Column(ints) => ints.units.get(n).copied().unwrap_or(0),
        Exact::Const(u, _) => *u,
    };
    // Units below 2^63 and powers below 2^60 multiply within 128 bits.
    let at = |e: &Exact<'_>, f: i128, n: usize| i128::from(unit(e, n)).saturating_mul(f);
    Some(judged(count, nulls, |n| Some(op.holds(at(x, fx, n).cmp(&at(y, fy, n))))))
}

/// The scale of exact numbers as integers.
fn scale_of(e: &Exact<'_>) -> u32 {
    match e {
        Exact::Column(ints) => ints.scale,
        Exact::Const(_, scale) => *scale,
    }
}

/// "`x op y`" of approximate numbers, character strings, datetimes or
/// truth values, each a column of its kind or a constant: none of any
/// other operands.
fn typed(op: CompOp, (x, y): (&Vector<'_>, &Vector<'_>), count: usize, nulls: Option<Mask>) -> Option<Bools> {
    let holds = |ord: Option<Ordering>| ord.map(|o| op.holds(o));
    match (x.column(), y.column(), x, y) {
        (Some(Column::Floats(a)), Some(Column::Floats(b)), ..) => {
            Some(judged(count, nulls, |n| holds(number(&a.numbers, n).partial_cmp(&number(&b.numbers, n)))))
        }
        (Some(Column::Floats(a)), None, _, Vector::Const(Value::Approx(c))) => {
            Some(judged(count, nulls, |n| holds(number(&a.numbers, n).partial_cmp(c))))
        }
        (None, Some(Column::Floats(b)), Vector::Const(Value::Approx(c)), _) => {
            Some(judged(count, nulls, |n| holds(c.partial_cmp(&number(&b.numbers, n)))))
        }
        (Some(Column::Texts(a)), Some(Column::Texts(b)), ..) => {
            Some(judged(count, nulls, |n| Some(op.holds(compare_strings(a.get(n).unwrap_or(""), b.get(n).unwrap_or(""))))))
        }
        (Some(Column::Texts(a)), None, _, Vector::Const(Value::Char(c))) => {
            Some(judged(count, nulls, |n| Some(op.holds(compare_strings(a.get(n).unwrap_or(""), c)))))
        }
        (None, Some(Column::Texts(b)), Vector::Const(Value::Char(c)), _) => {
            Some(judged(count, nulls, |n| Some(op.holds(compare_strings(c, b.get(n).unwrap_or(""))))))
        }
        (Some(Column::Datetimes(a)), Some(Column::Datetimes(b)), ..) => {
            let same = a.moment == b.moment;
            Some(judged(count, nulls, |n| same.then(|| op.holds(instant(&a.utc, n).cmp(&instant(&b.utc, n))))))
        }
        (Some(Column::Datetimes(a)), None, _, Vector::Const(Value::Datetime(c))) => {
            Some(judged(count, nulls, |n| (a.moment == c.moment).then(|| op.holds(instant(&a.utc, n).cmp(&c.utc)))))
        }
        (Some(Column::Bools(a)), Some(Column::Bools(b)), ..) => {
            Some(judged(count, nulls, |n| Some(op.holds(a.bits.get(n).cmp(&b.bits.get(n))))))
        }
        (Some(Column::Bools(a)), None, _, Vector::Const(Value::Bool(c))) => {
            Some(judged(count, nulls, |n| Some(op.holds(a.bits.get(n).cmp(c)))))
        }
        _ => None,
    }
}

/// The number of row `n`.
fn number(numbers: &[f64], n: usize) -> f64 {
    numbers.get(n).copied().unwrap_or(0.0)
}

/// The instant of row `n`.
fn instant(utc: &[i64], n: usize) -> i64 {
    utc.get(n).copied().unwrap_or(0)
}

/// [ISO/IEC 9075-2:2023 §8.15 GR1] Whether each row's `x` and `y` are
/// distinct, never unknown: none if a row's would raise.
pub(super) fn distinct(x: &Vector<'_>, y: &Vector<'_>, count: usize) -> Option<Vector<'static>> {
    let (a, b) = (x.seq(), y.seq());
    let made = (0..count).map(|n| value_distinct(a.get(n), b.get(n)).ok()).collect::<Option<Vec<bool>>>()?;
    Some(bools(judged(count, None, |n| made.get(n).copied())))
}

/// [X3.135-1986 §5.14 GR4, GR5] Whether each row's `x` matches
/// `pattern`, unknown of a null or of no pattern: none of a value not a
/// character string, which a row's evaluation reads otherwise. A vector's
/// text is searched at once by `kernels` for a piece between two `%`s
/// (`value/like/arena.rs`, RULES.md D1119).
pub(super) fn liked(x: &Vector<'_>, pattern: Option<&LikePattern>, count: usize, kernels: &dyn Kernels) -> Option<Vector<'static>> {
    let Some(pattern) = pattern else { return Some(Vector::Const(Value::Null)) };
    if let Some(Column::Texts(texts)) = x.column() {
        if texts.coded.is_none()
            && texts.ends.len() == count
            && let Some(all) = pattern.matches_all((texts.text.as_bytes(), &texts.ends), kernels)
        {
            return Some(bools(judged(count, texts.nulls.clone(), |n| all.get(n).copied())));
        }
        return Some(bools(judged(count, texts.nulls.clone(), |n| Some(texts.get(n).is_some_and(|s| pattern.matches(s))))));
    }
    let seq = x.seq();
    let mut made = Vec::with_capacity(count);
    for n in 0..count {
        made.push(match seq.get(n) {
            Value::Null => None,
            Value::Char(s) => Some(pattern.matches(s)),
            Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Lob(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Row(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Json(_)
            | Value::Xml(_)
            | Value::Extension(_) => return None,
        });
    }
    Some(bools(judged(count, None, |n| made.get(n).copied().flatten())))
}

/// [X3.135-1986 §5.13 GR2] Whether each row's `x` is IN `set`: none if a
/// row's would raise.
pub(super) fn within(x: &Vector<'_>, set: &InSet, count: usize) -> Option<Vector<'static>> {
    if let Some(Column::Texts(texts)) = x.column()
        && set.of_strings()
    {
        return Some(bools(texts_within(texts, set, count)));
    }
    let seq = x.seq();
    let made = (0..count).map(|n| set.truth_read(seq.get(n)).ok()).collect::<Option<Vec<Truth>>>()?;
    Some(bools(judged(count, None, |n| decided(made.get(n).copied().unwrap_or(Truth::Unknown)))))
}

/// \[DESIGN\] (RULES.md D1126) Whether each of `texts` is IN `set`, a
/// list of strings, by its text, no value made of it: a dictionary's
/// entries each judged once, while they are no more than the rows.
fn texts_within(texts: &Texts, set: &InSet, count: usize) -> Bools {
    let found = |s: Option<&str>| s.and_then(|s| set.text_truth(s)).and_then(decided);
    match texts.coded.as_deref().filter(|coded| coded.entries.len() <= count) {
        Some(coded) => {
            let entries: Vec<Option<bool>> = (0..coded.entries.len()).map(|e| found(coded.entries.get(e))).collect();
            let of_row = |n: usize| coded.codes.get(n).and_then(|&c| entries.get(index(c)).copied().flatten());
            judged(count, texts.nulls.clone(), |n| if texts.is_null(n) { None } else { of_row(n) })
        }
        None => judged(count, texts.nulls.clone(), |n| found(texts.get(n))),
    }
}

/// [X3.135-1986 §5.14 GR4, GR5] Whether each row's `x` matches its
/// `pattern`, ESCAPE its `escape` if given, as a row's evaluation matches
/// it, the escape judged first (`like_escape`), the patterns split by
/// `statement`: none if a row's would raise.
pub(super) fn liked_of(
    (x, pattern, escape): (&Vector<'_>, &Vector<'_>, Option<&Vector<'_>>),
    (count, statement): (usize, &dyn Subqueries),
) -> Option<Vector<'static>> {
    let (xs, patterns, escapes) = (x.seq(), pattern.seq(), escape.map(Vector::seq));
    let mut made = Vec::with_capacity(count);
    for n in 0..count {
        let character = match like_escape(escapes.as_ref().map(|e| e.get(n))).ok()? {
            Escape::None => None,
            Escape::Of(c) => Some(c),
            Escape::Null => {
                made.push(Truth::Unknown);
                continue;
            }
        };
        made.push(like_truth(xs.get(n), patterns.get(n), (character, statement)).ok()?);
    }
    Some(bools(judged(count, None, |n| decided(made.get(n).copied().unwrap_or(Truth::Unknown)))))
}
