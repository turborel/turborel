//! \[DESIGN\] (RULES.md D1122) A LIKE of a constant pattern judged of a
//! frozen row group's column at positions, ascending, of its codes alone,
//! as a filter judges a column given late. A chunk of texts coded by a
//! table of symbols (`fsst.rs`) is judged by the pattern's machine for its
//! table (`value/like/codes.rs`), no text decoded. A chunk coded any other
//! way, or a pattern the machine cannot take, gives no answer, and the
//! filter decodes the column as before: a dictionary's rows then as its
//! entries, each judged once (`exec/batch/judge/texts.rs`).

use super::super::encoded::Body;
use super::super::group::{Group, Seal};
use super::super::padding;
use super::Reading;
use crate::error::Result;
use crate::value::{DataType, LikePattern};

/// Of column `column`, of type `t`, of `group`, at `positions`, whether
/// each value matches `pattern`, unknown of a null; none if a chunk they
/// are in is not coded by symbols, the pattern has `_`, or a value's codes
/// are malformed, which decoding it then reports.
///
/// # Errors
///
/// As `Pages::with`.
pub fn liked_at<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    (positions, pattern): (&[u32], &LikePattern),
) -> Result<Option<Vec<Option<bool>>>> {
    // Symbols code VARCHAR alone, never padded.
    if padding(t) != Some(None) {
        return Ok(None);
    }
    let Some(zones) = group.columns.get(column) else { return Ok(None) };
    let mut out = Vec::with_capacity(positions.len());
    let mut rest = positions;
    for zone in zones {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let (these, after) = rest.split_at(rest.partition_point(|&p| p < end));
        rest = after;
        if these.is_empty() {
            continue;
        }
        let first = zone.run.first;
        let judged = prepared.with(pages, (column, zone, t), |chunk, _| {
            let Body::Fsst(coded) = &chunk.body else { return Ok(None) };
            let Some(machine) = pattern.on_codes(&coded.symbols()) else { return Ok(None) };
            for &p in these {
                let n = usize::try_from(p.saturating_sub(first)).unwrap_or(usize::MAX);
                if chunk.is_null(n) {
                    out.push(None);
                    continue;
                }
                let Some(matched) = coded.codes_of(n).and_then(|codes| machine.matches(codes)) else { return Ok(None) };
                out.push(Some(matched));
            }
            Ok(Some(()))
        })?;
        if judged.is_none() {
            return Ok(None);
        }
    }
    Ok((out.len() == positions.len()).then_some(out))
}
