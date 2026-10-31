//! [X3.135-1986 §8.3 GR3b] ORDER BY: rows sorted by each key in turn, most
//! significant first. \[DESIGN\] Rows that fit the work area are sorted in
//! memory; more are sorted externally (RULES.md D38): each work area's
//! worth sorted and written as a run, each row beside its first key's
//! prefix, the runs merged by their prefixes, as many at once as the work
//! area reads, in passes until one is left (`sort/merge.rs`). Sorting is
//! stable, a tie going to the earlier run, so rows of equal keys keep their
//! order. Rows are sorted by a prefix of their first key, where it has one
//! (`sort/prefix.rs`). The runs read back look for a cancel of the
//! statement (RULES.md D44). Split from exec.rs (RULES.md R-SIZE-2).

mod merge;
mod prefix;

use std::cmp::Ordering;
use std::sync::Arc;

use super::work::Work;
use super::{RowIter, broken, failed};
use crate::db::State;
use crate::error::Result;
use crate::logical::SortKey;
use crate::spill::{SpillRun, row_bytes};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::sort_cmp;
use merge::{Order, merge, record};

/// [X3.135-1986 §8.3 GR3b] `input` sorted by `keys`, in a work area of
/// `db`, for the statement of `tx`. Never inlined into the caller, whose
/// frame is on the stack while its input is made (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn sort(keys: &[SortKey], input: RowIter, db: &State, tx: TxId) -> RowIter {
    match sorted(keys, input, &mut Work::of(db, tx)) {
        Ok(rows) => rows,
        Err(e) => failed(e),
    }
}

/// `sort`'s work.
fn sorted(keys: &[SortKey], input: RowIter, work: &mut Work) -> Result<RowIter> {
    let (mut rows, mut bytes, mut runs) = (Vec::new(), 0_usize, Vec::new());
    for r in input {
        let row = r?;
        if let Some(k) = keys.iter().find(|k| row.get(k.column).is_none()) {
            return Err(broken(&format!("sort column {} is not in the rows sorted", k.column)));
        }
        bytes = bytes.saturating_add(row_bytes(&row));
        rows.push(row);
        if bytes > work.limit() {
            runs.push(written(keys, std::mem::take(&mut rows), work)?);
            bytes = 0;
        }
    }
    if runs.is_empty() {
        return Ok(Box::new(prefix::sorted(keys, rows).into_iter().map(Ok)));
    }
    if !rows.is_empty() {
        runs.push(written(keys, rows, work)?);
    }
    let (order, width) = (Arc::new(Order::new(keys, work.types().clone())), work.width());
    while runs.len() > width {
        let (mut merged, mut rest) = (Vec::new(), runs.into_iter());
        loop {
            let group: Vec<Box<dyn SpillRun>> = rest.by_ref().take(width).collect();
            if group.is_empty() {
                break;
            }
            let mut run = work.run()?;
            merge(&order, group)?.write_to(&mut *run)?;
            merged.push(run);
        }
        runs = merged;
    }
    Ok(Box::new(merge(&order, runs)?))
}

/// `rows`, sorted by `keys`, written as a run of `work`'s spill area, each
/// beside its prefix (`merge::record`).
fn written(keys: &[SortKey], rows: Vec<Row>, work: &mut Work) -> Result<Box<dyn SpillRun>> {
    let (mut run, mut out) = (work.run()?, Vec::new());
    for row in &prefix::sorted(keys, rows) {
        record(keys, row, &mut out);
        run.push(&out)?;
    }
    Ok(run)
}

/// [X3.135-1986 §8.3 GR3b] The order of rows `a` and `b` under `keys`.
/// `sort` has checked that both have every key column; a missing one would
/// compare equal.
pub fn compare_rows(keys: &[SortKey], a: &Row, b: &Row) -> Ordering {
    keys.iter()
        .map(|k| {
            let o = a.get(k.column).zip(b.get(k.column)).map_or(Ordering::Equal, |(x, y)| sort_cmp(x, y));
            if k.descending { o.reverse() } else { o }
        })
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}
