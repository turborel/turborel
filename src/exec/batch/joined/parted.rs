//! \[DESIGN\] An inner hash join of streams whose right operand outgrew its
//! work area, partitioned as the row path partitions it
//! (`exec/join/partitioned.rs`, RULES.md D38), where no order of its left
//! operand's is asked for (`in_order`): each pair of partitions is taken
//! in turn, its right built, and each row of its left probes it, the
//! joined rows put in batches as they are made. So no joined row is
//! written to a run and merged back into the left's order, as the row path
//! does to keep it.

use std::collections::VecDeque;

use super::super::stream::{Batches, Kept};
use crate::error::{Result, SqlError};
use crate::exec::RowIter;
use crate::exec::join::{Built, Partitions, Pending, each_pair};
use crate::exec::partition::unnumbered;
use crate::exec::work::Work;
use crate::spill::Records;
use crate::storage::{BATCH_ROWS, Batch, Row};
use crate::value::Value;
use crate::value::types::TypeRegistry;

/// The batches of the join of `left` with the partitioned right operand
/// `parts`, in `work`, of the left's columns `left_columns` and the right's
/// `right_columns`.
///
/// # Errors
///
/// The left operand's; the spill area's.
pub(super) fn joined(
    left: RowIter,
    parts: Partitions,
    mut work: Work,
    (left_columns, right_columns): (Vec<usize>, Vec<usize>),
) -> Result<Batches> {
    let pending = Pending::of(left, parts, &mut work)?;
    let types = work.types().clone();
    let width = left_columns.len().saturating_add(right_columns.len());
    let out = Batch::empty(width);
    Ok(Box::new(Parted {
        pending,
        work,
        types,
        current: None,
        columns: (left_columns, right_columns),
        out,
        ready: VecDeque::new(),
        failed: None,
        ended: false,
    }))
}

/// A partitioned join's batches, as they are pulled.
struct Parted {
    pending: Pending,
    work: Work,
    types: TypeRegistry,
    /// The pair of partitions being joined: its right built, and its
    /// left's rows still to probe it.
    current: Option<(Built, Records)>,
    /// The columns of the left's rows and of the right's a batch holds.
    columns: (Vec<usize>, Vec<usize>),
    /// The batch of joined rows being made.
    out: Batch,
    ready: VecDeque<Batch>,
    /// The error that ended it, once its batches are given.
    failed: Option<SqlError>,
    /// Whether it is done, or an error ended it.
    ended: bool,
}

impl Parted {
    /// Joins the next left row of the pair of partitions being joined, or
    /// takes up the next pair; the joined rows readied in batches.
    fn step(&mut self) -> Result<()> {
        let Some((built, probes)) = &mut self.current else {
            self.current = self.pending.next(&mut self.work)?;
            if self.current.is_none() {
                self.ended = true;
                self.flush();
            }
            return Ok(());
        };
        let Some(record) = probes.next() else {
            self.current = None;
            return Ok(());
        };
        let (_, l) = unnumbered(&record?, &self.types)?;
        let (out, ready, columns) = (&mut self.out, &mut self.ready, &self.columns);
        each_pair(built, &l, |r| {
            put(out, (&l, r), columns);
            if out.rows >= BATCH_ROWS {
                let width = out.columns.len();
                ready.push_back(std::mem::replace(out, Batch::empty(width)));
            }
            Ok(())
        })
    }
}

impl Parted {
    /// Readies the batch being made, if it holds a row.
    fn flush(&mut self) {
        if self.out.rows > 0 {
            let width = self.out.columns.len();
            self.ready.push_back(std::mem::replace(&mut self.out, Batch::empty(width)));
        }
    }
}

/// Puts in `out` the joined row of `l` and `r`: their values of `columns`,
/// the left's then the right's.
fn put(out: &mut Batch, (l, r): (&Row, &Row), (left_columns, right_columns): &(Vec<usize>, Vec<usize>)) {
    let values = left_columns.iter().map(|&c| l.get(c)).chain(right_columns.iter().map(|&c| r.get(c)));
    for (vector, v) in out.columns.iter_mut().zip(values) {
        vector.values_mut().push(v.cloned().unwrap_or(Value::Null));
    }
    out.rows = out.rows.saturating_add(1);
}

impl Iterator for Parted {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(batch) = self.ready.pop_front() {
                return Some(Ok((batch, None)));
            }
            if self.ended {
                return self.failed.take().map(Err);
            }
            // The rows joined before an error first, then the error.
            if let Err(e) = self.step() {
                self.flush();
                self.failed = Some(e);
                self.ended = true;
            }
        }
    }
}
