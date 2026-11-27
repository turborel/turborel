//! \[DESIGN\] Reading a frozen row group (RULES.md D31): the runs of its
//! rows the conditions of a scan may match, by the least and greatest values
//! and the nulls of each part of each chunk of the column each compares
//! (zone maps), and by the parts' Bloom filters; and the rows of a run, only
//! the columns asked for decoded, a chunk read once for the rows it holds.

use super::group::{Group, PART_ROWS, Pages, Seal};
use super::prepared::Prepared;
use super::{Chunk, bloom, imprints, padding};
use crate::error::Result;
use crate::storage::{Column, Conjunct, Conjunction, Datetimes, Floats, Ints, Mask, Row, Texts};
use crate::value::datetime::Moment;
use crate::value::types::narrow_scale;
use crate::value::{DataType, Value};

mod positions;
mod truths;

use positions::coded_at;
pub use positions::{column_at, datetimes_at, floats_at, ints_at, texts_at, values_at};
pub use truths::{bools, bools_at};

/// The pages a group's chunks are read of, and the chunks of the group
/// opened before, kept for the runs after (`prepared.rs`).
pub type Reading<'p, 'a, S> = (&'p Pages<'a, S>, &'p mut Prepared);

/// The runs of positions of `group`'s rows that each of `conditions` may
/// match, in order: those every one of them keeps (`kept_by`), every row if
/// there are none.
///
/// # Errors
///
/// SQLCODE -910 if `group`'s header was read without a column they read.
pub fn candidates(group: &Group, conditions: Option<&Conjunction>) -> Result<Vec<(u32, u32)>> {
    let mut runs = vec![(0, group.rows)];
    for condition in conditions.iter().flat_map(|c| c.conjuncts()) {
        runs = both(&runs, &kept_by(group, condition)?);
        if runs.is_empty() {
            break;
        }
    }
    Ok(runs)
}

/// As `candidates`, and of those, the runs of the parts whose Bloom
/// filters may hold a value an equality or IN of `conditions` asks for
/// (`bloom::kept`), their filters read from `pages`, and of a group with
/// imprints the runs those keep too (`read_ordered`).
///
/// # Errors
///
/// As `Pages::with`.
pub fn read_candidates<S: Seal>(group: &Group, pages: &Pages<'_, S>, conditions: Option<&Conjunction>) -> Result<Vec<(u32, u32)>> {
    read_ordered(group, pages, conditions).map(|(runs, _)| runs)
}

/// As `read_candidates`, narrowed further by imprints, each conjunct's
/// at the grain of their blocks (`imprints.rs`, RULES.md D511); and, of a
/// group with imprints, the places of `conditions`' conjuncts, narrowest
/// first, by the rows each alone keeps of the group by its zone maps and
/// imprints, so a scan judges first the one that keeps fewest.
///
/// # Errors
///
/// As `Pages::with`.
pub fn read_ordered<S: Seal>(
    group: &Group,
    pages: &Pages<'_, S>,
    conditions: Option<&Conjunction>,
) -> Result<(imprints::Runs, Vec<usize>)> {
    let mut runs = candidates(group, conditions)?;
    let Some(conditions) = conditions.filter(|_| !runs.is_empty()) else { return Ok((runs, Vec::new())) };
    for condition in conditions.conjuncts() {
        if runs.is_empty() {
            break;
        }
        if let Some(kept) = bloom::kept(group, pages, condition, &runs)? {
            runs = both(&runs, &kept);
        }
    }
    // Without imprints the order stays as written: by zone maps alone, a
    // conjunct keeping fewer parts may keep more rows, and cost more.
    if !group.imprinted() {
        return Ok((runs, Vec::new()));
    }
    let all = [(0, group.rows)];
    let mut kept = Vec::with_capacity(conditions.conjuncts().len());
    for (n, c) in conditions.conjuncts().iter().enumerate() {
        kept.push((n, rows_of(&kept_by(group, c)?)));
    }
    for (n, alone) in imprints::each_kept(group, pages, conditions, &all)? {
        if let Some(k) = kept.get_mut(n) {
            k.1 = rows_of(&alone);
        }
        if !runs.is_empty() {
            runs = both(&runs, &imprints::bridged(&alone));
        }
    }
    kept.sort_by_key(|&(_, rows)| rows);
    Ok((runs, kept.into_iter().map(|(n, _)| n).collect()))
}

/// The rows of `runs`.
fn rows_of(runs: &[(u32, u32)]) -> u64 {
    runs.iter().map(|(from, to)| u64::from(to.saturating_sub(*from))).sum()
}

/// The runs in both `a` and `b`, each in order and apart.
pub(super) fn both(a: &[(u32, u32)], b: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0_usize, 0_usize);
    while let (Some(&(a_from, a_to)), Some(&(b_from, b_to))) = (a.get(i), b.get(j)) {
        let (from, to) = (a_from.max(b_from), a_to.min(b_to));
        if from < to {
            out.push((from, to));
        }
        if a_to <= b_to {
            i = i.saturating_add(1);
        } else {
            j = j.saturating_add(1);
        }
    }
    out
}

/// The runs of positions of `group`'s rows that `condition` may match, in
/// order: those of the parts of its column's chunks, `PART_ROWS` rows
/// each, whose values may match: by their least and greatest values, or,
/// for IS NULL and IS NOT NULL, their nulls; of a conjunct of several
/// columns, those of the parts of each that it may match with the others'
/// values anywhere in the group (RULES.md D511); of a LIKE, every row.
fn kept_by(group: &Group, condition: &Conjunct) -> Result<Vec<(u32, u32)>> {
    // A LIKE passes over no part and reads no zone to say so, as the
    // bounds of its prefix, told beside it, do that (RULES.md D1251).
    if matches!(condition, Conjunct::Like { .. }) {
        return Ok(vec![(0, group.rows)]);
    }
    let columns = condition.columns();
    let width = columns.iter().max().map_or(0, |c| c.saturating_add(1));
    // Of a conjunct of several columns, each column's parts with the
    // others' bounds over the group: the rows of every one it may match.
    let mut runs = vec![(0, group.rows)];
    for &column in &columns {
        let mut bounds: Vec<Option<(Value, Value)>> = vec![None; width];
        for &other in columns.iter().filter(|&&o| o != column) {
            if let (Some(slot), Some(zones)) = (bounds.get_mut(other), group.zones(other)?) {
                *slot = imprints::whole(zones);
            }
        }
        runs = both(&runs, &kept_of(group, condition, (column, bounds))?);
    }
    Ok(runs)
}

/// The runs of positions of `group`'s rows, of the parts of `column`'s
/// chunks, `condition` may match, of `bounds` of the other columns it
/// reads.
fn kept_of(group: &Group, condition: &Conjunct, (column, mut bounds): (usize, Vec<Option<(Value, Value)>>)) -> Result<Vec<(u32, u32)>> {
    let column = &column;
    let Some(zones) = group.zones(*column)? else { return Ok(vec![(0, group.rows)]) };
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let parts = zones.iter().flat_map(|zone| {
        let end = zone.run.first.saturating_add(zone.run.rows);
        zone.parts.iter().enumerate().map(move |(n, part)| {
            let from = zone.run.first.saturating_add(u32::try_from(n).unwrap_or(u32::MAX).saturating_mul(PART_ROWS));
            (part, (from, from.saturating_add(PART_ROWS).min(end)))
        })
    });
    for (part, (from, to)) in parts {
        if let Some(b) = bounds.get_mut(*column) {
            b.clone_from(&part.bounds);
        }
        let may = match condition {
            Conjunct::Null { not: false, .. } => part.nulls > 0,
            Conjunct::Null { not: true, .. } => part.nulls < to.saturating_sub(from),
            Conjunct::Condition(_)
            | Conjunct::In { .. }
            | Conjunct::NotIn { .. }
            | Conjunct::Any(_)
            | Conjunct::Dynamic { .. }
            | Conjunct::Spans { .. }
            | Conjunct::Unpadded { .. }
            | Conjunct::Like { .. } => condition.may_match(&bounds),
        };
        if !may {
            continue;
        }
        // A run that goes on from the one before joins it.
        if let Some(last) = runs.last_mut().filter(|last| last.1 == from) {
            last.1 = to;
        } else {
            runs.push((from, to));
        }
    }
    Ok(runs)
}

/// The rows at positions `from` to `to` of `group`, its columns of
/// `types`, each as wide as the table, only `columns` holding their values
/// and the others null.
///
/// # Errors
///
/// As `Pages::get`; SQLCODE -916 if a chunk is malformed.
pub fn rows<S: Seal>(
    group: &Group,
    pages: &Pages<'_, S>,
    types: &[DataType],
    columns: &[usize],
    (from, to): (u32, u32),
) -> Result<Vec<Row>> {
    let count = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
    let mut out: Vec<Row> = (0..count).map(|_| vec![Value::Null; types.len()]).collect();
    for &column in columns {
        let (Some(zones), Some(t)) = (group.zones(column)?, types.get(column)) else { continue };
        for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
            pages.with_run(&zone.run, |bytes| fill(&Chunk::parse(bytes)?, (column, t), (zone.run.first, from, to), &mut out))?;
        }
    }
    Ok(out)
}

/// Puts the values of `chunk`, column `column` of type `t` of the rows from
/// position `first`, into those of `out`, the rows from `from`, that are
/// before `to`.
fn fill(chunk: &Chunk<'_>, (column, t): (usize, &DataType), (first, from, to): (u32, u32, u32), out: &mut [Row]) -> Result<()> {
    let rows = u32::try_from(chunk.len()).unwrap_or(u32::MAX);
    for position in first.max(from)..first.saturating_add(rows).min(to) {
        let value = chunk.value(usize::try_from(position.saturating_sub(first)).unwrap_or(usize::MAX), t)?;
        let slot = usize::try_from(position.saturating_sub(from)).ok().and_then(|at| out.get_mut(at)?.get_mut(column));
        if let Some(slot) = slot {
            *slot = value;
        }
    }
    Ok(())
}

/// The values of column `column`, of type `t`, of the rows at positions
/// `from` to `to` of `group`.
///
/// # Errors
///
/// As `rows`.
pub fn values<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Vec<Value>> {
    let count = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
    let mut out = Vec::with_capacity(count);
    let Some(zones) = group.zones(column)? else { return Ok(vec![Value::Null; count]) };
    for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
        let (first, end) = (zone.run.first.max(from), zone.run.first.saturating_add(zone.run.rows).min(to));
        prepared.with(pages, (column, zone, t), |chunk, _| {
            for position in first..end {
                out.push(chunk.value(usize::try_from(position.saturating_sub(zone.run.first)).unwrap_or(usize::MAX), t)?);
            }
            Ok(())
        })?;
    }
    Ok(out)
}

/// The 8-byte fields of a column, each as a signed integer, and whether
/// each is null: none if one of them is not so.
type Words = Option<(Vec<i64>, Mask)>;

/// 8-byte fields, whether each is null, and where each run of one value
/// among them ends, if known.
type RunWords = ((Vec<i64>, Mask), Option<Vec<usize>>);

/// Of nulls, none if none is null.
fn nulls_of(nulls: Mask) -> Option<Mask> {
    nulls.any().then_some(nulls)
}

/// `word`, an approximate number's 8 bytes as a signed integer, as the
/// number.
fn float_of(word: i64) -> f64 {
    f64::from_bits(u64::from_le_bytes(word.to_le_bytes()))
}

/// \[DESIGN\] The values of column `column`, of type `t`, of the rows at
/// positions `from` to `to` of `group`, as integers (`storage::Ints`), if
/// `t` is an exact numeric type of no more digits than 64 bits hold
/// (`narrow_scale`), which a record writes as its units in 8 bytes, and every chunk's encoding holds integers.
///
/// # Errors
///
/// As `rows`.
pub fn ints<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Option<Ints>> {
    let Some(scale) = narrow_scale(t) else { return Ok(None) };
    Ok(words(group, (pages, prepared), (column, t), (from, to))?.map(|((units, nulls), runs)| {
        let ints = Ints::new(units, scale, nulls_of(nulls));
        match runs {
            Some(ends) => ints.with_runs(ends),
            None => ints,
        }
    }))
}

/// \[DESIGN\] As `ints`, as approximate numbers (`storage::Floats`), if `t`
/// is an approximate numeric type, which a record writes as its bits in 8
/// bytes.
///
/// # Errors
///
/// As `rows`.
pub fn floats<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Option<Floats>> {
    let DataType::Approximate { .. } = t else { return Ok(None) };
    Ok(words(group, (pages, prepared), (column, t), (from, to))?
        .map(|((words, nulls), _)| Floats::new(words.into_iter().map(float_of).collect(), nulls_of(nulls))))
}

/// \[DESIGN\] (RULES.md D354) The fields, zone and precision of a datetime
/// type without a time zone, which a record writes as its instant's
/// microseconds in 8 bytes; none of any other type.
fn datetime_shape(t: &DataType) -> Option<(Moment, bool, u8)> {
    let (moment, precision) = match t {
        DataType::Date => (Moment::Date, 0),
        DataType::Time { precision, zoned: false } => (Moment::Time, *precision),
        DataType::Timestamp { precision, zoned: false } => (Moment::Timestamp, *precision),
        DataType::Time { zoned: true, .. }
        | DataType::Timestamp { zoned: true, .. }
        | DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Json
        | DataType::Xml(_)
        | DataType::Extension(_) => return None,
    };
    Some((moment, false, u8::try_from(precision).ok()?))
}

/// \[DESIGN\] As `ints`, as datetimes (`storage::Datetimes`), if `t` is a
/// datetime type without a time zone (`datetime_shape`).
///
/// # Errors
///
/// As `rows`.
pub fn datetimes<S: Seal>(
    group: &Group,
    reading: Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Option<Datetimes>> {
    let Some(shape) = datetime_shape(t) else { return Ok(None) };
    Ok(words(group, reading, (column, t), (from, to))?.map(|((utc, nulls), _)| Datetimes::new(utc, None, shape, nulls_of(nulls))))
}

/// The 8-byte fields of column `column` of the rows at positions `from` to
/// `to`, each chunk's read at once, and where each run of one value among
/// them ends, if every chunk keeps its values as runs, of the same reading
/// as their values (`Chunk::ints_ending`).
fn words<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Option<RunWords>> {
    let Some(zones) = group.zones(column)? else { return Ok(None) };
    let count = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
    let (mut units, mut nulls) = (Vec::with_capacity(count), Mask::default());
    let mut runs = Some(Vec::new());
    for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
        let (first, end) = (zone.run.first.max(from), zone.run.first.saturating_add(zone.run.rows).min(to));
        let at = |p: u32| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        let base = units.len();
        let held = prepared.with(pages, (column, zone, t), |chunk, made| {
            // Of runs, their values and ends of one reading (RULES.md D1171).
            let ran = match &mut runs {
                Some(ends) => chunk.ints_ending((at(first), at(end)), (&mut units, &mut nulls), (ends, base)),
                None => Some(false),
            };
            Ok(match ran {
                Some(true) => Some(()),
                Some(false) => {
                    runs = None;
                    chunk.ints_made((at(first), at(end)), (&mut units, &mut nulls), (made, pages.kernels))
                }
                None => None,
            })
        })?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if units.len() != count {
        return Ok(None);
    }
    Ok(Some(((units, nulls), runs)))
}

/// \[DESIGN\] The values of column `column`, of type `t`, of the rows at
/// positions `from` to `to` of `group`, as text, as `texts_at` gives them.
///
/// # Errors
///
/// As `rows`.
pub fn texts<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Option<Texts>> {
    if padding(t).is_none() {
        return Ok(None);
    }
    let Some(zones) = group.zones(column)? else { return Ok(None) };
    if let Some(coded) = coded_at((zones, column, t), (pages, &mut *prepared), &(from..to).collect::<Vec<u32>>())? {
        return Ok(Some(coded));
    }
    let count = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
    let (mut text, mut ends, mut nulls) = (String::new(), Vec::with_capacity(count), Mask::default());
    for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
        let (first, end) = (zone.run.first.max(from), zone.run.first.saturating_add(zone.run.rows).min(to));
        let at = |p: u32| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        let held = prepared.with(pages, (column, zone, t), |chunk, _| {
            Ok(chunk.texts((at(first), at(end)), (&mut text, &mut ends, &mut nulls), (t, pages.kernels)))
        })?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if ends.len() != count {
        return Ok(None);
    }
    Ok(Some(Texts::new(text, ends, nulls_of(nulls))))
}

/// \[DESIGN\] Column `c`, of type `t`, of the rows at positions `from` to
/// `to` of `group`: as integers, approximate numbers, datetimes, truth
/// values or text if it is of them, each chunk read a run at a time, else
/// as values.
///
/// # Errors
///
/// As `rows`.
pub fn column_of_run<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (c, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Column> {
    if let Some(ints) = ints(group, (pages, &mut *prepared), (c, t), (from, to))? {
        return Ok(Column::Ints(ints));
    }
    if let Some(floats) = floats(group, (pages, &mut *prepared), (c, t), (from, to))? {
        return Ok(Column::Floats(floats));
    }
    if let Some(datetimes) = datetimes(group, (pages, &mut *prepared), (c, t), (from, to))? {
        return Ok(Column::Datetimes(datetimes));
    }
    if let Some(bools) = bools(group, (pages, &mut *prepared), (c, t), (from, to))? {
        return Ok(Column::Bools(bools));
    }
    if let Some(texts) = texts(group, (pages, &mut *prepared), (c, t), (from, to))? {
        return Ok(Column::Texts(texts));
    }
    Ok(Column::from(values(group, (pages, prepared), (c, t), (from, to))?))
}
