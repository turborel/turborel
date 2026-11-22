//! \[DESIGN\] CASE over vectors (RULES.md D363), NULLIF and COALESCE
//! among them [ISO/IEC 9075:1992 §6.9]: the rows given split by each
//! WHEN's condition, the rows it is true of taking its result, the rest
//! going on to the next WHEN, the ELSE taking those none is true of
//! [GR2]. Each WHEN and each result is evaluated of its own rows alone,
//! their input vectors gathered for them, so a result no row takes, or a
//! WHEN no row reaches, is never evaluated and raises nothing, as a row's
//! evaluation does not evaluate it. The results are scattered back into
//! one vector, in the rows' order: of integers of one scale, of
//! approximate numbers, of character strings, if every result is, else
//! of values, each the value its result gave.

use super::vector::{Vector, gather};
use super::{Cx, Node, evaluated, logic};
use crate::storage::mask::index;
use crate::storage::{Column, Floats, Ints, Mask, Texts};
use crate::value::Value;

/// The rows a part of a CASE took, places among those given, and their
/// values.
type Piece = (Vec<u32>, Vector<'static>);

/// The CASE of `whens` and `otherwise` of every row of `cx`: none if a
/// row's evaluation would raise.
pub(super) fn case<'a>((whens, otherwise): (&[(Node, Node)], &Node), cx: &Cx<'a>) -> Option<Vector<'a>> {
    let all: Vec<u32> = (0..cx.count).map(|n| u32::try_from(n).unwrap_or(u32::MAX)).collect();
    let mut left = all;
    let mut pieces: Vec<Piece> = Vec::new();
    for (when, then) in whens {
        if left.is_empty() {
            break;
        }
        let truths = logic::truths(within(when, &left, cx)?, left.len());
        let (taken, rest): (Vec<u32>, Vec<u32>) = left.iter().enumerate().fold((Vec::new(), Vec::new()), |(mut t, mut r), (k, &p)| {
            if truths.bits.get(k) {
                t.push(p);
            } else {
                r.push(p);
            }
            (t, r)
        });
        if !taken.is_empty() {
            let values = within(then, &taken, cx)?;
            pieces.push((taken, values));
        }
        left = rest;
    }
    if !left.is_empty() {
        let values = within(otherwise, &left, cx)?;
        pieces.push((left, values));
    }
    Some(scattered(&pieces, cx.count))
}

/// The vector of `node` of the rows at `places` of those `cx` holds, its
/// inputs gathered for them unless they are all; owned.
fn within(node: &Node, places: &[u32], cx: &Cx<'_>) -> Option<Vector<'static>> {
    if places.len() == cx.count {
        return evaluated(node, cx).map(owned);
    }
    let gathered: Vec<Column> = cx.inputs.iter().map(|c| gather(c, places, cx.statement.vector_kernels())).collect();
    let sub = Cx { inputs: gathered.iter().collect(), count: places.len(), statement: cx.statement };
    evaluated(node, &sub).map(owned)
}

/// `v`, owned.
fn owned(v: Vector<'_>) -> Vector<'static> {
    match v {
        Vector::Column(c) => Vector::Owned(c.clone()),
        Vector::Owned(c) => Vector::Owned(c),
        Vector::Const(c) => Vector::Const(c),
    }
}

/// What every piece's values are, so the scattered vector is of their
/// kind.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ints(u32),
    Floats,
    Texts,
    Values,
}

/// The kind of `v`'s values, none of nulls alone, which every kind
/// holds.
fn kind_of(v: &Vector<'_>) -> Option<Kind> {
    match v {
        Vector::Const(c) => constant_kind(c),
        Vector::Column(c) => column_kind(c),
        Vector::Owned(c) => column_kind(c),
    }
}

/// The kind of a constant's value, none of the null value.
fn constant_kind(v: &Value) -> Option<Kind> {
    Some(match v {
        Value::Null => return None,
        Value::Exact(d) => i64::try_from(d.units).map_or(Kind::Values, |_| Kind::Ints(d.scale)),
        Value::Approx(_) => Kind::Floats,
        Value::Char(_) => Kind::Texts,
        Value::DecFloat(_)
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
        | Value::Extension(_) => Kind::Values,
    })
}

/// The kind of a column's values, none of nulls alone.
fn column_kind(c: &Column) -> Option<Kind> {
    Some(match c {
        Column::Ints(ints) => Kind::Ints(ints.scale),
        Column::Floats(_) => Kind::Floats,
        Column::Texts(_) => Kind::Texts,
        Column::Nulls(_) => return None,
        Column::Values(_) | Column::Datetimes(_) | Column::Bools(_) | Column::Late(_) => Kind::Values,
    })
}

/// The values of `pieces` in the order of the `count` rows they took.
fn scattered(pieces: &[Piece], count: usize) -> Vector<'static> {
    if let [(_, only)] = pieces {
        return match only {
            Vector::Const(v) => Vector::Const(v.clone()),
            Vector::Column(c) => Vector::Owned((*c).clone()),
            Vector::Owned(c) => Vector::Owned(c.clone()),
        };
    }
    let mut slots = vec![(0_usize, 0_usize); count];
    for (p, (places, _)) in pieces.iter().enumerate() {
        for (k, &place) in places.iter().enumerate() {
            if let Some(slot) = slots.get_mut(index(place)) {
                *slot = (p, k);
            }
        }
    }
    let kinds: Vec<Kind> = pieces.iter().filter_map(|(_, v)| kind_of(v)).collect();
    let Some((&first, rest)) = kinds.split_first() else { return Vector::Const(Value::Null) };
    let kind = if rest.iter().all(|k| *k == first) { first } else { Kind::Values };
    let value = |(p, k): (usize, usize)| pieces.get(p).map_or(Value::Null, |(_, v)| v.seq().get(k).clone());
    Vector::Owned(match kind {
        Kind::Ints(scale) => Column::Ints(scattered_ints(pieces, &slots, scale)),
        Kind::Floats => Column::Floats(scattered_floats(pieces, &slots)),
        Kind::Texts => Column::Texts(scattered_texts(pieces, &slots)),
        Kind::Values => Column::Values(slots.iter().map(|&s| value(s)).collect()),
    })
}

/// The integers of scale `scale` of `pieces` at `slots`.
fn scattered_ints(pieces: &[Piece], slots: &[(usize, usize)], scale: u32) -> Ints {
    let mut nulls = Mask::none(slots.len());
    let units = slots
        .iter()
        .enumerate()
        .map(|(n, &(p, k))| {
            let unit = match pieces.get(p).map(|(_, v)| v) {
                Some(Vector::Const(Value::Exact(d))) => i64::try_from(d.units).ok(),
                Some(Vector::Column(Column::Ints(ints)) | Vector::Owned(Column::Ints(ints))) => {
                    (!ints.is_null(k)).then(|| ints.units.get(k).copied()).flatten()
                }
                Some(Vector::Const(_) | Vector::Column(_) | Vector::Owned(_)) | None => None,
            };
            if unit.is_none() {
                nulls.set(n);
            }
            unit.unwrap_or(0)
        })
        .collect();
    Ints::new(units, scale, nulls.any().then_some(nulls))
}

/// The approximate numbers of `pieces` at `slots`.
fn scattered_floats(pieces: &[Piece], slots: &[(usize, usize)]) -> Floats {
    let mut nulls = Mask::none(slots.len());
    let numbers = slots
        .iter()
        .enumerate()
        .map(|(n, &(p, k))| {
            let number = match pieces.get(p).map(|(_, v)| v) {
                Some(Vector::Const(Value::Approx(f))) => Some(*f),
                Some(Vector::Column(Column::Floats(floats)) | Vector::Owned(Column::Floats(floats))) => floats.get(k),
                Some(Vector::Const(_) | Vector::Column(_) | Vector::Owned(_)) | None => None,
            };
            if number.is_none() {
                nulls.set(n);
            }
            number.unwrap_or(0.0_f64)
        })
        .collect();
    Floats::new(numbers, nulls.any().then_some(nulls))
}

/// The character strings of `pieces` at `slots`.
fn scattered_texts(pieces: &[Piece], slots: &[(usize, usize)]) -> Texts {
    let (mut text, mut ends, mut nulls) = (String::new(), Vec::with_capacity(slots.len()), Mask::none(slots.len()));
    for (n, &(p, k)) in slots.iter().enumerate() {
        let s = match pieces.get(p).map(|(_, v)| v) {
            Some(Vector::Const(Value::Char(s))) => Some(s.as_str()),
            Some(Vector::Column(Column::Texts(texts)) | Vector::Owned(Column::Texts(texts))) => texts.get(k),
            Some(Vector::Const(_) | Vector::Column(_) | Vector::Owned(_)) | None => None,
        };
        match s {
            Some(s) => text.push_str(s),
            None => nulls.set(n),
        }
        ends.push(text.len());
    }
    Texts::new(text, ends, nulls.any().then_some(nulls))
}
