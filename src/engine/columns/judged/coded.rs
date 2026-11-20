//! \[DESIGN\] (RULES.md D360) A conjunct of a column of texts coded by a
//! table of symbols (`fsst.rs`) judged in place, no text decoded, as
//! Boncz, Neumann and Leis, "FSST" (PVLDB 2020), §5, compare a constant
//! with compressed strings. FSST encodes the constant with the same table
//! and compares the code strings, which is exact only for strings compared
//! octet by octet; SQL compares strings padded with spaces to one length
//! [X3.135-1986 §5.11 GR5], so `'a'` equals `'a '`, whose codes need not
//! begin with `'a'`'s. Each text's symbols are compared instead with the
//! constant's bytes as they are read, the first that differs deciding
//! (`Symbols::compare_padded`), which orders as `compare_strings` does and
//! reads, of most texts, a code or two.

use std::cell::Cell;

use super::super::encoded::Body;
use super::super::group::{Group, Seal};
use super::super::scan::Reading;
use super::super::{Chunk, padding};
use super::{Pred, text_keeps};
use crate::error::Result;
use crate::storage::mask::index;
use crate::value::DataType;
use std::cmp::Ordering;

/// The rows of `given`, places from `from`, or of all of the run `from` to
/// `to` of `group`, that `pred` keeps, of a column of type `t`, if every
/// chunk the run reads of it is coded by symbols and the conjunct is
/// judged of text (`text_keeps`): none if not, the column then decoded.
///
/// # Errors
///
/// As decoding a chunk.
pub(super) fn kept<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (pred, t): (&Pred, &DataType),
    (from, to): (u32, u32),
    given: Option<&[u32]>,
) -> Result<Option<Vec<u32>>> {
    // Of VARCHAR alone, never padded, as the table codes no other.
    if padding(t) != Some(None) || text_keeps(&pred.conjunct, &|_| Some(Ordering::Equal)).is_none() {
        return Ok(None);
    }
    let Some(zones) = group.columns.get(pred.column) else { return Ok(None) };
    let all = super::rows(usize::try_from(to.saturating_sub(from)).unwrap_or(0), given);
    let mut out = Vec::with_capacity(all.len());
    let mut rest = all.as_slice();
    for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
        let end = zone.run.first.saturating_add(zone.run.rows).saturating_sub(from);
        let (these, after) = rest.split_at(rest.partition_point(|&r| r < end));
        rest = after;
        if these.is_empty() {
            continue;
        }
        let first = zone.run.first;
        let judged = prepared.with(pages, (pred.column, zone, t), |chunk, _| Ok(judged(chunk, pred, (first, from), these, &mut out)))?;
        if judged.is_none() {
            return Ok(None);
        }
    }
    Ok(Some(out))
}

/// Pushes to `out` the rows of `these`, places from `from`, that `pred`
/// keeps of `chunk`, whose first row is `first`: none if it is not coded
/// by symbols, or a text's codes are malformed, so it is decoded and
/// raises what is malformed as it would have.
fn judged(chunk: &Chunk<'_>, pred: &Pred, (first, from): (u32, u32), these: &[u32], out: &mut Vec<u32>) -> Option<()> {
    let Body::Fsst(coded) = &chunk.body else { return None };
    let failed = Cell::new(false);
    for &r in these {
        let n = index(from.saturating_add(r).saturating_sub(first));
        if chunk.is_null(n) {
            if pred.of_null {
                out.push(r);
            }
            continue;
        }
        let order = |c: &str| {
            let o = coded.compare_padded(n, c.as_bytes());
            failed.set(failed.get() || o.is_none());
            o
        };
        let keeps = text_keeps(&pred.conjunct, &order)?;
        if failed.get() {
            return None;
        }
        if keeps {
            out.push(r);
        }
    }
    Some(())
}
