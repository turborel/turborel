//! \[DESIGN\] RULES.md D380: a sort's rows given as batches, so an operator
//! that reads batches reads a sort's as it reads a scan's. Each batch of up
//! to `BATCH_ROWS` rows in order is gathered of the batches the sort held,
//! a column of each a vector of the kind every batch held keeps it as, of
//! one scale or type alike: integers, approximate numbers, texts, datetimes
//! and truth values; any other, or of kinds not alike, its values. No row
//! is made. A column of texts in every batch held, of a store's
//! dictionaries in one at least, is gathered as codes of one dictionary
//! set up once for the sort (`Dictionaries`): each store's dictionary
//! once, and each other batch's texts as entries of their own, so a text
//! is copied at most once, and one of a store's dictionaries not at all.

use std::sync::Arc;

use crate::storage::{Batch, Bools, Coded, Column, Datetimes, Floats, Ints, Mask, Texts};
use crate::value::datetime::Moment;

/// A row picked: its batch's place among those held, and its place there.
pub(super) type Placed = (u32, u32);

/// What a column's vector is, for whether two are alike: its kind, its
/// scale of integers, its type of datetimes; none of values.
type Shape = (u8, u32, Option<(Moment, bool, u8)>);

/// Texts set one after another, and of each batch where its entries begin
/// among them and whether its rows are codes, else each row its entry.
type Dictionary = (Arc<Texts>, Vec<(u32, bool)>);

/// \[DESIGN\] Of each column of the batches a sort held, if every one keeps
/// it as texts, one at least of a store's dictionaries: each store's dictionary once and each other batch's texts,
/// one after another, and where each batch's begin, so a row of any batch
/// is its code, or its place, moved by that.
pub(super) struct Dictionaries(Vec<Option<Dictionary>>);

impl Dictionaries {
    /// Those of the first `width` columns of `batches`.
    pub(super) fn of(batches: &[(Batch, Vec<usize>)], width: usize) -> Dictionaries {
        Dictionaries((0..width).map(|c| dictionary(batches, c)).collect())
    }
}

/// Column `c`'s dictionary of `batches`, if every one keeps it as texts and
/// one at least as a store's.
fn dictionary(batches: &[(Batch, Vec<usize>)], c: usize) -> Option<Dictionary> {
    let (mut text, mut ends, mut starts) = (String::new(), Vec::new(), Vec::with_capacity(batches.len()));
    let mut seen: Vec<(Arc<Texts>, u32)> = Vec::new();
    for (batch, _) in batches {
        let Some(Column::Texts(texts)) = batch.columns.get(c) else { return None };
        let at = u32::try_from(ends.len()).ok()?;
        let Some(coded) = texts.coded.as_ref() else {
            put(texts, (&mut text, &mut ends));
            starts.push((at, false));
            continue;
        };
        if let Some(&(_, before)) = seen.iter().find(|(entries, _)| Arc::ptr_eq(entries, &coded.entries)) {
            starts.push((before, true));
            continue;
        }
        put(&coded.entries, (&mut text, &mut ends));
        seen.push((Arc::clone(&coded.entries), at));
        starts.push((at, true));
    }
    // Of none of a store's, the texts are gathered as they are.
    (!seen.is_empty()).then(|| (Arc::new(Texts::new(text, ends, None)), starts))
}

/// Appends each of `texts` to `buffer` as an entry, a null's empty.
fn put(texts: &Texts, (buffer, ends): (&mut String, &mut Vec<usize>)) {
    for n in 0..texts.len() {
        buffer.push_str(texts.get(n).unwrap_or_default());
        ends.push(buffer.len());
    }
}

/// The rows at `picks` of `batches`, in order, each `width` columns wide,
/// a column of `dictionaries` gathered as codes.
pub(super) fn picked(batches: &[(Batch, Vec<usize>)], picks: &[Placed], width: usize, dictionaries: &Dictionaries) -> Batch {
    let columns = (0..width)
        .map(|c| match dictionaries.0.get(c) {
            Some(Some(dictionary)) => coded_of(batches, c, picks, dictionary),
            Some(None) | None => column_of(batches, c, picks),
        })
        .collect();
    Batch { columns, rows: picks.len() }
}

/// Column `c` of the rows at `picks` of `batches`, every one's texts, as
/// codes among `dictionary`'s entries.
fn coded_of(batches: &[(Batch, Vec<usize>)], c: usize, picks: &[Placed], (entries, starts): &Dictionary) -> Column {
    let (mut codes, mut nulls) = (Vec::with_capacity(picks.len()), Mask::none(picks.len()));
    for (n, &(b, r)) in picks.iter().enumerate() {
        let at = usize::try_from(b).unwrap_or(usize::MAX);
        let r = usize::try_from(r).unwrap_or(usize::MAX);
        let texts = batches.get(at).and_then(|(batch, _)| if let Some(Column::Texts(t)) = batch.columns.get(c) { Some(t) } else { None });
        let start = starts.get(at);
        let code = texts.filter(|t| !t.is_null(r) && r < t.len()).zip(start).and_then(|(t, &(_, coded))| match t.coded.as_ref() {
            Some(dict) => dict.codes.get(r).copied().filter(|_| coded),
            None => u32::try_from(r).ok().filter(|_| !coded),
        });
        if let Some((code, &(start, _))) = code.zip(start) {
            codes.push(code.saturating_add(start));
        } else {
            codes.push(0);
            nulls.set(n);
        }
    }
    Column::Texts(Texts::coded(Coded { codes, entries: Arc::clone(entries) }, nulls.any().then_some(nulls)))
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
