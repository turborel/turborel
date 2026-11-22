//! \[DESIGN\] The rows a join of streams makes, gathered into batches
//! (RULES.md D31): of each joined row, the place of its left row in the
//! batch probing, or none for nulls, and its right row's values. A batch is
//! made of them once it holds `BATCH_ROWS`, or the batch probing is done:
//! each left vector gathered as one of its kind (`Column::gathered`), so a
//! column passing through the join unchanged keeps its integers, numbers or
//! texts; each right vector of values, or, of a right operand held as
//! vectors (`held.rs`), gathered of its vector so too; integers gathered
//! on the statement's vector kernels (RULES.md D570). A batch whose probe
//! raises an error gives the rows joined before it first, then the error,
//! as a join read row by row would (RULES.md D15).

use std::collections::VecDeque;
use std::sync::Arc;

use super::super::stream::Kept;
use crate::error::{Result, SqlError};
use crate::kernels::Kernels;
use crate::storage::{BATCH_ROWS, Batch, Column, NO_ROW, Picks, Row};
use crate::value::Value;

/// The joined rows of a join of streams, as they are made, and the batches
/// of them made and not yet given.
pub(super) struct Out {
    /// Of each joined row not yet in a batch, its left row's place, or
    /// `NO_ROW`.
    left: Vec<u32>,
    /// Of each column of the right operand's rows a joined batch holds,
    /// the joined rows' values.
    right: Vec<Vec<Value>>,
    /// The vectors of a left batch.
    left_width: usize,
    /// The column of the right operand's rows each right vector holds.
    right_columns: Vec<usize>,
    /// The right operand held as vectors, if it is, and of each joined row
    /// not yet in a batch, its right row's place there, or `NO_ROW`.
    held: Option<(Arc<Batch>, Vec<u32>)>,
    ready: VecDeque<Batch>,
    failed: Option<SqlError>,
    /// The kernels its integers are gathered on.
    kernels: Arc<dyn Kernels>,
}

/// The place `p` as a pick: `NO_ROW` of none, or of one past it.
fn pick(p: Option<usize>) -> u32 {
    p.and_then(|p| u32::try_from(p).ok()).unwrap_or(NO_ROW)
}

impl Out {
    /// No joined rows yet, of batches of `left_width` left vectors, then
    /// the right's of `right_columns`, gathered on `kernels`.
    pub(super) fn new(left_width: usize, right_columns: Vec<usize>, kernels: Arc<dyn Kernels>) -> Out {
        let right = right_columns.iter().map(|_| Vec::with_capacity(BATCH_ROWS)).collect();
        let left = Vec::with_capacity(BATCH_ROWS);
        Out { left, right, left_width, right_columns, held: None, ready: VecDeque::new(), failed: None, kernels }
    }

    /// No joined rows yet, of batches of `left_width` left vectors, then
    /// each vector of `right`, a right operand held as vectors, gathered on
    /// `kernels`.
    pub(super) fn over(left_width: usize, right: Arc<Batch>, kernels: Arc<dyn Kernels>) -> Out {
        let mut out = Out::new(left_width, Vec::new(), kernels);
        out.held = Some((right, Vec::with_capacity(BATCH_ROWS)));
        out
    }

    /// Adds a joined row: row `l` of `batch`, or nulls; then row `r` of the
    /// right operand held as vectors, or nulls. A batch is made of the rows
    /// once they are `BATCH_ROWS`.
    pub(super) fn push_held(&mut self, batch: Option<&Batch>, l: Option<usize>, r: Option<usize>) {
        if let Some((_, picks)) = &mut self.held {
            picks.push(pick(r));
        }
        self.push(batch, l, None);
    }

    /// Adds a joined row: row `l` of `batch`, or nulls; then `right`, or
    /// nulls. A batch is made of the rows once they are `BATCH_ROWS`.
    pub(super) fn push(&mut self, batch: Option<&Batch>, l: Option<usize>, right: Option<&Row>) {
        self.left.push(pick(l));
        for (vector, &c) in self.right.iter_mut().zip(&self.right_columns) {
            vector.push(right.and_then(|row| row.get(c)).cloned().unwrap_or(Value::Null));
        }
        if self.left.len() >= BATCH_ROWS {
            self.flush(batch);
        }
    }

    /// Makes a batch of the joined rows not yet in one, if any, their left
    /// rows those of `batch`.
    pub(super) fn flush(&mut self, batch: Option<&Batch>) {
        if self.left.is_empty() {
            return;
        }
        let (rows, kernels, left) = (self.left.len(), &*self.kernels, Picks::new(&self.left));
        let mut columns: Vec<Column> = (0..self.left_width)
            .map(|at| batch.and_then(|b| b.columns.get(at)).map_or_else(|| Column::nulls(rows), |c| c.gathered_by(&left, kernels)))
            .collect();
        columns.extend(self.right.iter_mut().map(|v| Column::Values(std::mem::replace(v, Vec::with_capacity(BATCH_ROWS)))));
        if let Some((right, picks)) = &mut self.held {
            let picks_of = Picks::new(picks);
            columns.extend(right.columns.iter().map(|c| c.gathered_by(&picks_of, kernels)));
            drop(picks_of);
            picks.clear();
        }
        drop(left);
        self.left.clear();
        self.ready.push_back(Batch { columns, rows });
    }

    /// Makes a batch of the joined rows not yet in one, their left rows
    /// those of `batch`, once it is probed: `done`, or failed, the error
    /// then given after them. Whether it failed.
    pub(super) fn settle(&mut self, batch: Option<&Batch>, done: Result<()>) -> bool {
        self.flush(batch);
        self.failed = done.err();
        self.failed.is_some()
    }

    /// Whether a batch is made and not yet given.
    pub(super) fn ready(&self) -> bool {
        !self.ready.is_empty()
    }

    /// The next batch made, or once all are given, the error raised, if
    /// one was.
    pub(super) fn next(&mut self) -> Option<Result<Kept>> {
        match self.ready.pop_front() {
            Some(batch) => Some(Ok((batch, None))),
            None => self.failed.take().map(Err),
        }
    }
}

/// The places in a batch of the rows a filter kept of it, `rows`, or of
/// all its `count` rows.
pub(super) fn kept_rows(rows: Option<crate::storage::Sel>, count: usize) -> Vec<usize> {
    crate::storage::sel::places_of(rows, count)
}
