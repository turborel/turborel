//! \[DESIGN\] Reading a frozen row group (RULES.md D31): the runs of its
//! rows the conditions of a scan may match, by the least and greatest values of each
//! chunk of the column each compares (zone maps), and the rows of a run, only
//! the columns asked for decoded, a chunk read once for the rows it holds.

use super::Chunk;
use super::group::{Group, Pages, Seal};
use crate::error::Result;
use crate::storage::{Conjunct, Conjunction, Row};
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
/// chunks of its column whose values may match: by their least and
/// greatest values, or, for IS NULL and IS NOT NULL, their nulls.
fn kept_by(group: &Group, condition: &Conjunct) -> Vec<(u32, u32)> {
    let all = vec![(0, group.rows)];
    let columns = condition.columns();
    let [column] = columns.as_slice() else { return all };
    let Some(zones) = group.columns.get(*column) else { return all };
    let mut bounds: Vec<Option<(Value, Value)>> = vec![None; column.saturating_add(1)];
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for zone in zones {
        if let Some(b) = bounds.get_mut(*column) {
            b.clone_from(&zone.bounds);
        }
        let may = match condition {
            Conjunct::Null { not: false, .. } => zone.nulls > 0,
            Conjunct::Null { not: true, .. } => zone.nulls < zone.run.rows,
            Conjunct::Condition(_) | Conjunct::In { .. } | Conjunct::NotIn { .. } | Conjunct::Any(_) => condition.may_match(&bounds),
        };
        if !may {
            continue;
        }
        let (from, to) = (zone.run.first, zone.run.first.saturating_add(zone.run.rows));
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
