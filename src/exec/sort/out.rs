//! \[DESIGN\] RULES.md D380: a sort's rows given as batches, so an operator
//! that reads batches reads a sort's as it reads a scan's. Each batch of up
//! to `BATCH_ROWS` rows in order is gathered of the batches the sort held,
//! a column of each a vector of the kind every batch held keeps it as, of
//! one scale or type alike: integers, approximate numbers, texts, datetimes
//! and truth values; any other, or of kinds not alike, its values. No row
//! is made.

use crate::storage::{Batch, Bools, Column, Datetimes, Floats, Ints, Mask, Texts};
use crate::value::datetime::Moment;

/// A row picked: its batch's place among those held, and its place there.
pub(super) type Placed = (u32, u32);

/// What a column's vector is, for whether two are alike: its kind, its
/// scale of integers, its type of datetimes; none of values.
type Shape = (u8, u32, Option<(Moment, bool, u8)>);

/// The rows at `picks` of `batches`, in order, each `width` columns wide.
pub(super) fn picked(batches: &[(Batch, Vec<usize>)], picks: &[Placed], width: usize) -> Batch {
    let columns = (0..width).map(|c| column_of(batches, c, picks)).collect();
    Batch { columns, rows: picks.len() }
}

/// The shape of `column`'s vector; none of values, nulls alone, or a late
/// column.
fn shape(column: &Column) -> Option<Shape> {
    match column {
        Column::Ints(i) => Some((1, i.scale, None)),
        Column::Floats(_) => Some((2, 0, None)),
        Column::Texts(_) => Some((3, 0, None)),
        Column::Datetimes(d) => Some((4, 0, Some((d.moment, d.zoned, d.precision)))),
        Column::Bools(_) => Some((5, 0, None)),
        Column::Values(_) | Column::Nulls(_) | Column::Late(_) => None,
    }
}

/// Column `c` of the rows at `picks` of `batches`: a vector of the kind
/// every batch holds it as, alike, else their values.
fn column_of(batches: &[(Batch, Vec<usize>)], c: usize, picks: &[Placed]) -> Column {
    let of: Vec<Option<&Column>> = batches.iter().map(|(b, _)| b.columns.get(c)).collect();
    let first = of.iter().flatten().next().and_then(|f| shape(f));
    let alike = first.is_some() && of.iter().all(|o| o.and_then(shape) == first);
    match of.iter().flatten().next().copied().filter(|_| alike) {
        Some(Column::Ints(i)) => {
            let (units, nulls) = gather(&of, picks, |col, r| {
                if let Column::Ints(ints) = col { ints.units.get(r).filter(|_| !ints.is_null(r)).copied() } else { None }
            });
            Column::Ints(Ints::new(units, i.scale, nulls))
        }
        Some(Column::Floats(_)) => {
            let (numbers, nulls) = gather(&of, picks, |col, r| if let Column::Floats(f) = col { f.get(r) } else { None });
            Column::Floats(Floats::new(numbers, nulls))
        }
        Some(Column::Texts(_)) => texts_of(&of, picks),
        Some(Column::Datetimes(d)) => datetimes_of(&of, picks, d),
        Some(Column::Bools(_)) => {
            let (truths, nulls) = gather(&of, picks, |col, r| if let Column::Bools(b) = col { b.get(r) } else { None });
            let mut bits = Mask::none(truths.len());
            for (n, _) in truths.iter().enumerate().filter(|(_, t)| **t) {
                bits.set(n);
            }
            Column::Bools(Bools::new(bits, nulls))
        }
        Some(Column::Values(_) | Column::Nulls(_) | Column::Late(_)) | None => values_of(batches, c, picks),
    }
}

/// Each of `picks` as its batch's column and the row's place.
fn each<'a>(of: &'a [Option<&'a Column>], picks: &'a [Placed]) -> impl Iterator<Item = (Option<&'a Column>, usize)> + 'a {
    picks.iter().map(move |&(b, r)| {
        let column = usize::try_from(b).ok().and_then(|b| of.get(b).copied().flatten());
        (column, usize::try_from(r).unwrap_or(usize::MAX))
    })
}

/// The values `get` reads of the rows at `picks`, a null's the default,
/// and which are null, if any is.
fn gather<T: Default>(of: &[Option<&Column>], picks: &[Placed], get: impl Fn(&Column, usize) -> Option<T>) -> (Vec<T>, Option<Mask>) {
    let (mut values, mut nulls) = (Vec::with_capacity(picks.len()), Mask::none(picks.len()));
    for (n, (column, r)) in each(of, picks).enumerate() {
        if let Some(v) = column.and_then(|col| get(col, r)) {
            values.push(v);
        } else {
            values.push(T::default());
            nulls.set(n);
        }
    }
    (values, nulls.any().then_some(nulls))
}

/// The texts of the rows at `picks`, copied into one buffer.
fn texts_of(of: &[Option<&Column>], picks: &[Placed]) -> Column {
    let (mut text, mut ends, mut nulls) = (String::new(), Vec::with_capacity(picks.len()), Mask::none(picks.len()));
    for (n, (column, r)) in each(of, picks).enumerate() {
        match column.and_then(|col| if let Column::Texts(t) = col { t.get(r) } else { None }) {
            Some(t) => text.push_str(t),
            None => nulls.set(n),
        }
        ends.push(text.len());
    }
    Column::Texts(Texts::new(text, ends, nulls.any().then_some(nulls)))
}

/// The datetimes of the rows at `picks`, of the type of `like`.
fn datetimes_of(of: &[Option<&Column>], picks: &[Placed], like: &Datetimes) -> Column {
    let read = |col: &Column, r: usize| {
        let Column::Datetimes(d) = col else { return None };
        let utc = d.utc.get(r).filter(|_| !d.is_null(r))?;
        Some((*utc, d.zones.as_ref().and_then(|z| z.get(r)).copied().unwrap_or(0)))
    };
    let (instants, nulls) = gather(of, picks, read);
    let zones = like.zoned.then(|| instants.iter().map(|&(_, z)| z).collect());
    let utc = instants.into_iter().map(|(u, _)| u).collect();
    Column::Datetimes(Datetimes::new(utc, zones, (like.moment, like.zoned, like.precision), nulls))
}

/// Column `c`'s values of the rows at `picks` of `batches`.
fn values_of(batches: &[(Batch, Vec<usize>)], c: usize, picks: &[Placed]) -> Column {
    let value = |&(b, r): &Placed| {
        let batch = usize::try_from(b).ok().and_then(|b| batches.get(b));
        batch.map_or(crate::value::Value::Null, |(batch, _)| batch.cell(c, usize::try_from(r).unwrap_or(usize::MAX)).to_value())
    };
    Column::Values(picks.iter().map(value).collect())
}
