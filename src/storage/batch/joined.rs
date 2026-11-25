//! \[DESIGN\] (RULES.md D38, D1053) One vector of many batches' vectors
//! at one place, the rows of each at the places beside it, in order, as
//! a join holds its built operand (`held.rs`): of the kind every part's
//! is, integers of one scale, approximate numbers, datetimes of one type,
//! truth values, character strings or nulls, else of values. A vector of
//! words is made of its parts' words, never of a value made for each row,
//! a part all of whose rows are kept, in order, copied whole; the nulls
//! are looked for only where a part may hold one.

use super::{Bools, Column, Datetimes, Floats, Ints, Mask, Texts};
use crate::value::Value;
use crate::value::datetime::Moment;

/// A part: its vector, if its batch has one at the place, and the places
/// of its rows kept.
type Part<'p> = (Option<&'p Column>, &'p [usize]);

/// The vector of `rows` rows of each part's column, if it has one, at the
/// places beside it, in order: of the kind every part's is, as
/// `Batch::joined` says.
pub(super) fn joined_column(parts: &[Part<'_>], rows: usize) -> Column {
    if let Some(typed) = of_words(parts, rows) {
        return typed;
    }
    let each = || parts.iter().flat_map(|&(column, places)| places.iter().map(move |&r| (column, r)));
    let all = |kind: fn(&Column) -> bool| parts.iter().all(|(c, _)| c.is_some_and(kind));
    if all(|c| matches!(c, Column::Texts(_))) {
        let (mut text, mut ends) = (String::new(), Vec::with_capacity(rows));
        for (c, r) in each() {
            if let Some(Column::Texts(texts)) = c
                && let Some(s) = texts.get(r)
            {
                text.push_str(s);
            }
            ends.push(text.len());
        }
        return Column::Texts(Texts::new(text, ends, nulls(parts)));
    }
    if all(|c| matches!(c, Column::Nulls(_))) {
        return Column::nulls(rows);
    }
    Column::Values(each().map(|(c, r)| c.map_or(Value::Null, |c| c.value_at(r))).collect())
}

/// The vector of the parts' words, if every part's is of one kind of
/// words: integers of one scale, approximate numbers, datetimes of one
/// type, or truth values.
fn of_words(parts: &[Part<'_>], rows: usize) -> Option<Column> {
    let all = |kind: fn(&Column) -> bool| parts.iter().all(|(c, _)| c.is_some_and(kind));
    let scales: Vec<Option<u32>> = parts.iter().map(|(c, _)| c.and_then(Column::scale)).collect();
    if let Some(&Some(scale)) = scales.first()
        && scales.iter().all(|&s| s == Some(scale))
    {
        return Some(Column::Ints(Ints::new(words(parts, rows, units_of), scale, nulls(parts))));
    }
    if all(|c| matches!(c, Column::Floats(_))) {
        return Some(Column::Floats(Floats::new(words(parts, rows, numbers_of), nulls(parts))));
    }
    if let Some((kind, zoned)) = datetimes_of(parts) {
        let zones = zoned.then(|| words(parts, rows, zones_of));
        return Some(Column::Datetimes(Datetimes::new(words(parts, rows, utc_of), zones, kind, nulls(parts))));
    }
    if all(|c| matches!(c, Column::Bools(_))) {
        return Some(Column::Bools(Bools::new(truths(parts), nulls(parts))));
    }
    None
}

/// Whether `places` are all of `len` rows, in order: the part is kept
/// whole.
fn whole(places: &[usize], len: usize) -> bool {
    places.len() == len && places.iter().enumerate().all(|(n, &r)| n == r)
}

/// The words `of` gives of each part's vector, at its places, in order,
/// `rows` in all; a part kept whole copied at once, and a part with no
/// such words given the default for each of its rows.
fn words<T: Copy + Default>(parts: &[Part<'_>], rows: usize, of: fn(&Column) -> Option<&[T]>) -> Vec<T> {
    let mut out = Vec::with_capacity(rows);
    for &(column, places) in parts {
        let held = column.and_then(of).unwrap_or_default();
        if whole(places, held.len()) {
            out.extend_from_slice(held);
        } else {
            out.extend(places.iter().map(|&r| held.get(r).copied().unwrap_or_default()));
        }
    }
    out
}

/// The units of a vector of integers.
fn units_of(column: &Column) -> Option<&[i64]> {
    if let Column::Ints(ints) = column { Some(&ints.units) } else { None }
}

/// The numbers of a vector of approximate numbers.
fn numbers_of(column: &Column) -> Option<&[f64]> {
    if let Column::Floats(floats) = column { Some(&floats.numbers) } else { None }
}

/// The instants of a vector of datetimes.
fn utc_of(column: &Column) -> Option<&[i64]> {
    if let Column::Datetimes(datetimes) = column { Some(&datetimes.utc) } else { None }
}

/// The displacements of a vector of datetimes that holds them.
fn zones_of(column: &Column) -> Option<&[i16]> {
    if let Column::Datetimes(datetimes) = column { datetimes.zones.as_deref() } else { None }
}

/// The type of the parts' datetimes, and whether they hold displacements,
/// if every part's vector is of datetimes of one type, all holding
/// displacements or none.
fn datetimes_of(parts: &[Part<'_>]) -> Option<((Moment, bool, u8), bool)> {
    let kind = |column: Option<&Column>| match column {
        Some(Column::Datetimes(d)) => Some(((d.moment, d.zoned, d.precision), d.zones.is_some())),
        Some(_) | None => None,
    };
    let first = kind(parts.first()?.0)?;
    parts.iter().all(|&(column, _)| kind(column) == Some(first)).then_some(first)
}

/// The truth of each row of the parts' vectors of truth values, at its
/// places, in order; a part kept whole added a word at a time.
fn truths(parts: &[Part<'_>]) -> Mask {
    let mut bits = Mask::none(0);
    for &(column, places) in parts {
        let Some(Column::Bools(bools)) = column else { continue };
        if whole(places, bools.bits.len()) {
            bits.append(&bools.bits);
        } else {
            for &r in places {
                bits.push(r < bools.bits.len() && bools.bits.get(r));
            }
        }
    }
    bits
}

/// Whether `column` may hold a null: a typed vector with no mask of
/// nulls holds none.
fn may_hold_null(column: &Column) -> bool {
    match column {
        Column::Ints(ints) => ints.nulls.is_some(),
        Column::Floats(floats) => floats.nulls.is_some(),
        Column::Texts(texts) => texts.nulls.is_some(),
        Column::Datetimes(datetimes) => datetimes.nulls.is_some(),
        Column::Bools(bools) => bools.nulls.is_some(),
        Column::Values(_) | Column::Nulls(_) | Column::Late(_) => true,
    }
}

/// Which of the parts' rows are null, if any is: a row of a part with no
/// vector is. No row is looked at unless some part may hold a null.
fn nulls(parts: &[Part<'_>]) -> Option<Mask> {
    if !parts.iter().any(|(column, _)| column.is_none_or(may_hold_null)) {
        return None;
    }
    let each = parts.iter().flat_map(|&(column, places)| places.iter().map(move |&r| (column, r)));
    super::mask::mask_of(&each.map(|(c, r)| c.is_none_or(|c| c.null_at(r))).collect::<Vec<bool>>())
}

// Unit tests: vectors of words joined (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// Dates of `utc`, null where `nulls` says.
    fn dates(utc: Vec<i64>, nulls: Option<Vec<bool>>) -> Column {
        Column::Datetimes(Datetimes::new(utc, None, (Moment::Date, false, 0), nulls.map(Into::into)))
    }

    /// Truth values of `bits`, null where `nulls` says.
    fn truths_of(bits: &[bool], nulls: Option<Vec<bool>>) -> Column {
        Column::Bools(Bools::new(Mask::from_bools(bits), nulls.map(Into::into)))
    }

    /// Which rows of `nulls` are null.
    fn marked(nulls: Option<&Mask>) -> Vec<bool> {
        nulls.map(Mask::to_bools).unwrap_or_default()
    }

    /// Verifies: [DESIGN] RULES.md D1053: datetimes of one type and truth
    /// values joined stay vectors of their kind, a part kept whole and a
    /// part picked out of order alike, each row's word and null its own;
    /// parts with no null give no mask.
    ///
    /// Decision: every part's vector is of datetimes, of one type. Each
    /// alone makes the joined vector one of values: a part of another
    /// type, and a part with no vector at the place.
    #[test]
    fn datetimes_and_truth_values_stay_vectors_of_words() {
        let (first, second) = (dates(vec![10, 20, 30], None), dates(vec![40, 50, 60], Some(vec![false, false, true])));
        let joined = joined_column(&[(Some(&first), &[0, 1, 2]), (Some(&second), &[2, 0])], 5);
        let Column::Datetimes(d) = &joined else { panic!("not datetimes: {joined:?}") };
        assert_eq!(d.utc, [10, 20, 30, 60, 40]);
        assert_eq!(marked(d.nulls.as_ref()), [false, false, false, true, false]);
        let plain = joined_column(&[(Some(&first), &[0, 1, 2]), (Some(&first), &[1])], 4);
        assert!(matches!(&plain, Column::Datetimes(same) if same.nulls.is_none() && same.utc == [10, 20, 30, 20]), "{plain:?}");
        let (yes, no) = (truths_of(&[true, false, true], None), truths_of(&[false, true, true], Some(vec![true, false, false])));
        let marks = joined_column(&[(Some(&yes), &[0, 1, 2]), (Some(&no), &[2, 0])], 5);
        let Column::Bools(b) = &marks else { panic!("not truth values: {marks:?}") };
        assert_eq!(b.bits.to_bools(), [true, false, true, true, false]);
        assert_eq!(marked(b.nulls.as_ref()), [false, false, false, false, true]);
        let stamps = Column::Datetimes(Datetimes::new(vec![7], None, (Moment::Timestamp, false, 6), None));
        assert!(matches!(joined_column(&[(Some(&first), &[0]), (Some(&stamps), &[0])], 2), Column::Values(_)), "two types");
        let lacking = joined_column(&[(Some(&first), &[0]), (None, &[0])], 2);
        assert!(matches!(&lacking, Column::Values(v) if v.get(1).is_some_and(Value::is_null)), "{lacking:?}");
    }

    /// Verifies: [DESIGN] RULES.md D1053: integers and approximate numbers
    /// joined are their parts' words, a part kept whole copied as it is, a
    /// place past a part's rows a 0.
    #[test]
    fn numbers_are_joined_of_their_words() {
        let ints = |units: Vec<i64>| Column::Ints(Ints::new(units, 2, None));
        let (a, b) = (ints(vec![1, 2, 3]), ints(vec![4, 5]));
        let joined = joined_column(&[(Some(&a), &[0, 1, 2]), (Some(&b), &[1, 0, 9])], 6);
        assert!(matches!(&joined, Column::Ints(i) if i.units == [1, 2, 3, 5, 4, 0] && i.scale == 2 && i.nulls.is_none()), "{joined:?}");
        let floats = Column::Floats(Floats::new(vec![1.5_f64, 2.5_f64], Some(vec![false, true].into())));
        let numbers = joined_column(&[(Some(&floats), &[1, 0])], 2);
        let Column::Floats(f) = &numbers else { panic!("not numbers: {numbers:?}") };
        assert_eq!(marked(f.nulls.as_ref()), [true, false]);
        assert!(f.get(1).is_some_and(|x| (x - 1.5_f64).abs() < f64::EPSILON));
    }
}
