//! \[DESIGN\] Reading a frozen row group (RULES.md D31): the runs of its
//! rows the conditions of a scan may match, by the least and greatest values of each
//! chunk of the column each compares (zone maps), and the rows of a run, only
//! the columns asked for decoded, a chunk read once for the rows it holds.

use super::group::{Group, PART_ROWS, Pages, Seal, Zone};
use super::{Chunk, bloom};
use crate::error::Result;
use crate::storage::{Coded, Conjunct, Conjunction, Floats, Ints, Row, Texts};
use crate::value::{DataType, Value};

/// The runs of positions of `group`'s rows that each of `conditions` may
/// match, in order: those every one of them keeps (`kept_by`), every row if
/// there are none.
pub fn candidates(group: &Group, conditions: Option<&Conjunction>) -> Vec<(u32, u32)> {
    let mut runs = vec![(0, group.rows)];
    for condition in conditions.iter().flat_map(|c| c.conjuncts()) {
        runs = both(&runs, &kept_by(group, condition));
        if runs.is_empty() {
            break;
        }
    }
    runs
}

/// As `candidates`, and of those, the runs of the parts whose Bloom
/// filters may hold a value an equality or IN of `conditions` asks for
/// (`bloom::kept`), their filters read from `pages`.
///
/// # Errors
///
/// As `Pages::with`.
pub fn read_candidates<S: Seal>(group: &Group, pages: &Pages<'_, S>, conditions: Option<&Conjunction>) -> Result<Vec<(u32, u32)>> {
    let mut runs = candidates(group, conditions);
    for condition in conditions.iter().flat_map(|c| c.conjuncts()) {
        if runs.is_empty() {
            break;
        }
        if let Some(kept) = bloom::kept(group, pages, condition, &runs)? {
            runs = both(&runs, &kept);
        }
    }
    Ok(runs)
}

/// The runs in both `a` and `b`, each in order and apart.
fn both(a: &[(u32, u32)], b: &[(u32, u32)]) -> Vec<(u32, u32)> {
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
/// order: every row if it reads more than one column; else those of the
/// parts of its column's chunks, `PART_ROWS` rows each, whose values may
/// match: by their least and greatest values, or, for IS NULL and IS NOT
/// NULL, their nulls.
fn kept_by(group: &Group, condition: &Conjunct) -> Vec<(u32, u32)> {
    let all = vec![(0, group.rows)];
    let columns = condition.columns();
    let [column] = columns.as_slice() else { return all };
    let Some(zones) = group.columns.get(*column) else { return all };
    let mut bounds: Vec<Option<(Value, Value)>> = vec![None; column.saturating_add(1)];
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
            Conjunct::Condition(_) | Conjunct::In { .. } | Conjunct::NotIn { .. } | Conjunct::Any(_) => condition.may_match(&bounds),
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
    runs
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
        let (Some(zones), Some(t)) = (group.columns.get(column), types.get(column)) else { continue };
        for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
            pages.with(zone.run.page, |bytes| fill(&Chunk::parse(bytes)?, (column, t), (zone.run.first, from, to), &mut out))?;
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
pub fn values<S: Seal>(group: &Group, pages: &Pages<'_, S>, (column, t): (usize, &DataType), (from, to): (u32, u32)) -> Result<Vec<Value>> {
    let count = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
    let mut out = Vec::with_capacity(count);
    let Some(zones) = group.columns.get(column) else { return Ok(vec![Value::Null; count]) };
    for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
        let (first, end) = (zone.run.first.max(from), zone.run.first.saturating_add(zone.run.rows).min(to));
        pages.with(zone.run.page, |bytes| {
            let chunk = Chunk::parse(bytes)?;
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
type Words = Option<(Vec<i64>, Vec<bool>)>;

/// Of nulls, none if none is null.
fn nulls_of(nulls: Vec<bool>) -> Option<Vec<bool>> {
    nulls.contains(&true).then_some(nulls)
}

/// `word`, an approximate number's 8 bytes as a signed integer, as the
/// number.
fn float_of(word: i64) -> f64 {
    f64::from_bits(u64::from_le_bytes(word.to_le_bytes()))
}

/// \[DESIGN\] The values of column `column`, of type `t`, of the rows at
/// positions `from` to `to` of `group`, as integers (`storage::Ints`), if
/// `t` is an exact numeric type, which a record writes as its units in 8
/// bytes, and every chunk's encoding holds integers.
///
/// # Errors
///
/// As `rows`.
pub fn ints<S: Seal>(group: &Group, pages: &Pages<'_, S>, (column, t): (usize, &DataType), (from, to): (u32, u32)) -> Result<Option<Ints>> {
    let DataType::Exact { scale, .. } = t else { return Ok(None) };
    Ok(words(group, pages, column, (from, to))?.map(|(units, nulls)| Ints::new(units, *scale, nulls_of(nulls))))
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
    pages: &Pages<'_, S>,
    (column, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Option<Floats>> {
    let DataType::Approximate { .. } = t else { return Ok(None) };
    Ok(words(group, pages, column, (from, to))?
        .map(|(words, nulls)| Floats::new(words.into_iter().map(float_of).collect(), nulls_of(nulls))))
}

/// \[DESIGN\] As `ints_at`, as approximate numbers, as `floats` gives
/// those of a run.
///
/// # Errors
///
/// As `rows`.
pub fn floats_at<S: Seal>(
    group: &Group,
    pages: &Pages<'_, S>,
    (column, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Option<Floats>> {
    let DataType::Approximate { .. } = t else { return Ok(None) };
    Ok(words_at(group, pages, column, positions)?
        .map(|(words, nulls)| Floats::new(words.into_iter().map(float_of).collect(), nulls_of(nulls))))
}

/// The 8-byte fields of column `column` of the rows at positions `from` to
/// `to`, each chunk's read at once.
fn words<S: Seal>(group: &Group, pages: &Pages<'_, S>, column: usize, (from, to): (u32, u32)) -> Result<Words> {
    let Some(zones) = group.columns.get(column) else { return Ok(None) };
    let count = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
    let (mut units, mut nulls) = (Vec::with_capacity(count), Vec::with_capacity(count));
    for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
        let (first, end) = (zone.run.first.max(from), zone.run.first.saturating_add(zone.run.rows).min(to));
        let at = |p: u32| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        let held = pages.with(zone.run.page, |bytes| Ok(Chunk::parse(bytes)?.ints((at(first), at(end)), (&mut units, &mut nulls))))?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if units.len() != count {
        return Ok(None);
    }
    Ok(Some((units, nulls)))
}

/// \[DESIGN\] The values of column `column`, of type `t`, of the rows at
/// `positions` of `group`, in increasing order, as integers, as `ints`
/// gives those of a run: each chunk's span from the first position asked
/// of it to the last decoded at once, and those asked taken of it.
///
/// # Errors
///
/// As `rows`.
pub fn ints_at<S: Seal>(group: &Group, pages: &Pages<'_, S>, (column, t): (usize, &DataType), positions: &[u32]) -> Result<Option<Ints>> {
    let DataType::Exact { scale, .. } = t else { return Ok(None) };
    Ok(words_at(group, pages, column, positions)?.map(|(units, nulls)| Ints::new(units, *scale, nulls_of(nulls))))
}

/// The 8-byte fields of column `column` of the rows at `positions`, in
/// increasing order, each chunk's span from the first asked of it to the
/// last read at once.
fn words_at<S: Seal>(group: &Group, pages: &Pages<'_, S>, column: usize, positions: &[u32]) -> Result<Words> {
    let Some(zones) = group.columns.get(column) else { return Ok(None) };
    let (mut units, mut nulls) = (Vec::with_capacity(positions.len()), Vec::with_capacity(positions.len()));
    let (mut span, mut span_nulls) = (Vec::new(), Vec::new());
    let mut rest = positions;
    for zone in zones {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let (these, after) = rest.split_at(rest.partition_point(|&p| p < end));
        rest = after;
        let (Some(&low), Some(&high)) = (these.first(), these.last()) else { continue };
        let at = |p: u32| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        span.clear();
        span_nulls.clear();
        let held = pages.with(zone.run.page, |bytes| {
            Ok(Chunk::parse(bytes)?.ints((at(low), at(high).saturating_add(1)), (&mut span, &mut span_nulls)))
        })?;
        if held.is_none() {
            return Ok(None);
        }
        for &p in these {
            let n = at(p).saturating_sub(at(low));
            units.push(span.get(n).copied().unwrap_or(0));
            nulls.push(span_nulls.get(n).copied().unwrap_or(true));
        }
    }
    if units.len() != positions.len() {
        return Ok(None);
    }
    Ok(Some((units, nulls)))
}

/// \[DESIGN\] The values of column `column`, of type `t`, of the rows at
/// `positions` of `group`, in increasing order, as text (`storage::Texts`),
/// if `t` is a VARCHAR type, which a record writes as its text, and every
/// chunk's encoding holds text: each position's read in place.
///
/// # Errors
///
/// As `rows`.
pub fn texts_at<S: Seal>(group: &Group, pages: &Pages<'_, S>, (column, t): (usize, &DataType), positions: &[u32]) -> Result<Option<Texts>> {
    let DataType::Varying { .. } = t else { return Ok(None) };
    let Some(zones) = group.columns.get(column) else { return Ok(None) };
    if let Some(coded) = coded_at(zones, pages, positions)? {
        return Ok(Some(coded));
    }
    let (mut text, mut ends, mut nulls) = (String::new(), Vec::with_capacity(positions.len()), Vec::with_capacity(positions.len()));
    let mut rest = positions;
    for zone in zones {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let (these, after) = rest.split_at(rest.partition_point(|&p| p < end));
        rest = after;
        if these.is_empty() {
            continue;
        }
        let at = |p: u32| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        let held = pages.with(zone.run.page, |bytes| {
            let chunk = Chunk::parse(bytes)?;
            Ok(these.iter().try_for_each(|&p| chunk.texts((at(p), at(p).saturating_add(1)), (&mut text, &mut ends, &mut nulls))))
        })?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if ends.len() != positions.len() {
        return Ok(None);
    }
    let nulls = nulls.contains(&true).then_some(nulls);
    Ok(Some(Texts::new(text, ends, nulls)))
}

/// \[DESIGN\] The values at `positions` of a column whose chunks are
/// `zones`, as their dictionaries' entries and each row's entry among them
/// (`storage::Coded`), every chunk's entries one after another, if every
/// chunk asked of is a dictionary.
fn coded_at<S: Seal>(zones: &[Zone], pages: &Pages<'_, S>, positions: &[u32]) -> Result<Option<Texts>> {
    let (mut text, mut ends) = (String::new(), Vec::new());
    let (mut codes, mut nulls) = (Vec::with_capacity(positions.len()), Vec::with_capacity(positions.len()));
    let mut rest = positions;
    for zone in zones {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let (these, after) = rest.split_at(rest.partition_point(|&p| p < end));
        rest = after;
        if these.is_empty() {
            continue;
        }
        let at: Vec<usize> = these.iter().map(|&p| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX)).collect();
        let held =
            pages.with(zone.run.page, |bytes| Ok(Chunk::parse(bytes)?.coded(&at, (&mut text, &mut ends, &mut codes, &mut nulls))))?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if codes.len() != positions.len() {
        return Ok(None);
    }
    let nulls = nulls.contains(&true).then_some(nulls);
    Ok(Some(Texts::coded(Coded { codes, entries: Texts::new(text, ends, None) }, nulls)))
}

/// \[DESIGN\] The values of column `column`, of type `t`, of the rows at
/// positions `from` to `to` of `group`, as text, as `texts_at` gives them.
///
/// # Errors
///
/// As `rows`.
pub fn texts<S: Seal>(
    group: &Group,
    pages: &Pages<'_, S>,
    (column, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Option<Texts>> {
    let DataType::Varying { .. } = t else { return Ok(None) };
    let Some(zones) = group.columns.get(column) else { return Ok(None) };
    if let Some(coded) = coded_at(zones, pages, &(from..to).collect::<Vec<u32>>())? {
        return Ok(Some(coded));
    }
    let count = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
    let (mut text, mut ends, mut nulls) = (String::new(), Vec::with_capacity(count), Vec::with_capacity(count));
    for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
        let (first, end) = (zone.run.first.max(from), zone.run.first.saturating_add(zone.run.rows).min(to));
        let at = |p: u32| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        let held =
            pages.with(zone.run.page, |bytes| Ok(Chunk::parse(bytes)?.texts((at(first), at(end)), (&mut text, &mut ends, &mut nulls))))?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if ends.len() != count {
        return Ok(None);
    }
    let nulls = nulls.contains(&true).then_some(nulls);
    Ok(Some(Texts::new(text, ends, nulls)))
}

/// The values of column `column`, of type `t`, of the rows at `positions`
/// of `group`, in increasing order.
///
/// # Errors
///
/// As `rows`.
pub fn values_at<S: Seal>(group: &Group, pages: &Pages<'_, S>, (column, t): (usize, &DataType), positions: &[u32]) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(positions.len());
    let Some(zones) = group.columns.get(column) else { return Ok(vec![Value::Null; positions.len()]) };
    let mut rest = positions;
    for zone in zones {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let here = rest.partition_point(|&p| p < end);
        let (these, after) = rest.split_at(here);
        rest = after;
        if these.is_empty() {
            continue;
        }
        pages.with(zone.run.page, |bytes| {
            let chunk = Chunk::parse(bytes)?;
            for &position in these {
                out.push(chunk.value(usize::try_from(position.saturating_sub(zone.run.first)).unwrap_or(usize::MAX), t)?);
            }
            Ok(())
        })?;
    }
    Ok(out)
}
