//! Bounded output lookahead for non-raising spatial joins. Small groups
//! amortize transitions between geometry evaluation and cursor fetching
//! without materializing the complete result (D15, D38, D82). A refused
//! reservation preserves the current row and resumes direct streaming.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::error::{Result, SqlError};
use crate::exec::RowIter;
use crate::logical::Expr;
use crate::memory::{MemoryBudget, Reservation};
use crate::physical::Plan;
use crate::spill::row_bytes;
use crate::storage::Row;
use crate::txn::{Stop, checked};

/// The geometry-shape benchmarks compare this lookahead with individual
/// fetches and full materialization. It caps retained rows between fetches.
const ROWS: usize = 64;

/// Column projections add no evaluation whose timing lookahead changes.
pub(super) fn eligible(mut plan: &Plan) -> bool {
    while let Plan::Project { exprs, input, .. } = plan {
        if exprs.iter().any(|expr| !matches!(expr, Expr::Column(_))) {
            return false;
        }
        plan = input;
    }
    matches!(plan, Plan::IndexJoin { spatial: true, outer: false, .. })
}

pub(super) fn read(input: RowIter, budget: &Arc<MemoryBudget>, limit: usize, stop: Option<Arc<Stop>>) -> RowIter {
    let rows = buffered(input, budget, limit);
    // A cancel while filling a group must fail this fetch immediately,
    // and remain an error on later fetches rather than look like EOF.
    Box::new(checked(rows, stop))
}

fn buffered(input: RowIter, budget: &Arc<MemoryBudget>, limit: usize) -> RowIter {
    let bytes = headers(ROWS);
    if bytes > limit {
        return input;
    }
    let Ok(mut held) = budget.reserve(bytes, "spatial cursor output lookahead") else { return input };
    let mut rows = VecDeque::new();
    let allocated = rows.try_reserve_exact(ROWS);
    if allocated.is_err() {
        return input;
    }
    let header = headers(rows.capacity());
    if header > limit {
        return input;
    }
    let resized = held.resize(header);
    if resized.is_err() {
        return input;
    }
    Box::new(Buffered {
        input,
        rows,
        overflow: None,
        failure: None,
        ended: false,
        started: false,
        batching: true,
        held,
        header,
        used: header,
        limit,
    })
}

fn headers(rows: usize) -> usize {
    size_of::<Buffered>().saturating_add(rows.saturating_mul(size_of::<Row>()))
}

struct Buffered {
    input: RowIter,
    rows: VecDeque<Row>,
    // The current row that did not fit is retained once, as in an
    // ordinary streaming operator, and follows the reserved prefix.
    overflow: Option<Row>,
    failure: Option<SqlError>,
    ended: bool,
    started: bool,
    batching: bool,
    held: Reservation,
    header: usize,
    used: usize,
    limit: usize,
}

impl Buffered {
    fn direct(&mut self) -> Option<Result<Row>> {
        let row = self.input.next();
        self.ended = row.as_ref().is_none_or(Result::is_err);
        row
    }

    fn reserve_row(&mut self, row: &Row) -> bool {
        let size = row_bytes(row);
        let used = self.used.saturating_add(size);
        if used > self.limit {
            return false;
        }
        if used > self.held.bytes() {
            // Forecast from this row to avoid a shared-budget lock for
            // every ordinary row. Larger later rows are checked again.
            let rest = ROWS.saturating_sub(self.rows.len());
            let forecast = self.used.saturating_add(size.saturating_mul(rest)).min(self.limit);
            let reserved = self.held.resize(forecast);
            if reserved.is_err() {
                return false;
            }
        }
        self.used = used;
        true
    }

    fn fill(&mut self) {
        self.used = self.header;
        for _ in 0..ROWS {
            let row = match self.input.next() {
                Some(Ok(row)) => row,
                Some(Err(error)) => {
                    self.failure = Some(error);
                    self.ended = true;
                    return;
                }
                None => {
                    self.ended = true;
                    return;
                }
            };
            let reserved = self.reserve_row(&row);
            if !reserved {
                self.overflow = Some(row);
                self.batching = false;
                return;
            }
            self.rows.push_back(row);
        }
    }
}

impl Iterator for Buffered {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.started {
            self.started = true;
            return self.direct();
        }
        if self.rows.is_empty() && self.overflow.is_none() && !self.ended {
            if !self.batching {
                return self.direct();
            }
            self.fill();
        }
        let row = self.rows.pop_front().or_else(|| self.overflow.take());
        if let Some(row) = row {
            return Some(Ok(row));
        }
        self.failure.take().map(Err)
    }
}

#[cfg(test)]
mod tests;
