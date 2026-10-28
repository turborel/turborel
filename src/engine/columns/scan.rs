//! \[DESIGN\] Reading a frozen row group (RULES.md D31): the runs of its
//! rows a condition may match, by the least and greatest values of each
//! chunk of the column it compares (zone maps), and the rows of a run, only
//! the columns asked for decoded, a chunk read once for the rows it holds.

use super::Chunk;
use super::group::{Group, Pages, Seal};
use crate::error::Result;
use crate::storage::{Condition, Row};
use crate::value::{DataType, Value};

/// The runs of positions of `group`'s rows that `condition` may match, in
/// order: every row if there is none, or it compares more than one column;
/// else those of the chunks of its column whose values may match.
pub fn candidates(group: &Group, condition: Option<&Condition>) -> Vec<(u32, u32)> {
    let all = vec![(0, group.rows)];
    let Some(condition) = condition else { return all };
    let [column] = condition.columns.as_slice() else { return all };
    let Some(zones) = group.columns.get(*column) else { return all };
    let mut bounds: Vec<Option<(Value, Value)>> = vec![None; column.saturating_add(1)];
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for zone in zones {
        if let Some(b) = bounds.get_mut(*column) {
            b.clone_from(&zone.bounds);
        }
        if !condition.may_match(&bounds) {
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
