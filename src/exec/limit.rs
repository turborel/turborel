//! [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] OFFSET and FETCH FIRST run: the
//! counts evaluated once, as the rows are first asked for, then the first
//! rows of the input passed over and the rest kept up to the count, in
//! their order, and of WITH TIES the peers of the last kept after it. No
//! row past those kept is read, so an input that gives its rows as it
//! reads them, as a scan in an index's order does, reads no more. A
//! percentage is of every row of the input, so its rows are held first,
//! up to MEMORY.WORK_BYTES and then spilled (RULES.md D38), and counted.

mod percent;

use std::cmp::Ordering;
use std::sync::Arc;

use super::held::HeldRows;
use super::sort::compare_rows;
use super::subquery::{Runner, Subqueries};
use super::{RowIter, broken, eval, failed};
use crate::db::State;
use crate::error::{Code, Result, SqlState, fail_with};
use crate::logical::{Expr, Limit, SortKey};
use crate::physical::Plan;
use crate::spill::ListSpace;
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Value;

/// The rows the limit `plan` keeps.
#[inline(never)]
pub(super) fn limited(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Limit { limit, input } = plan else { return failed(broken("a limit is not one")) };
    match kept(&limit, &Runner::new(db, tx), super::execute(*input, db, tx), db.list_space()) {
        Ok(rows) => rows,
        Err(e) => failed(e),
    }
}

/// The rows `limit` keeps of `rows`, its counts evaluated with
/// `subqueries`, a percentage's rows held in `space`.
///
/// # Errors
///
/// SQLSTATE 2201X if the offset is below 0 [GR6b], 2201W if the count of
/// FETCH FIRST is below 1 [GR7c], either null as well, being no number of
/// rows; their evaluation's; of a percentage, the input's, and holding its
/// rows'.
fn kept(limit: &Limit, subqueries: &dyn Subqueries, rows: RowIter, space: ListSpace) -> Result<RowIter> {
    let skip = match &limit.offset {
        Some(e) => count(e, subqueries, &Rule::OFFSET)?,
        None => 0,
    };
    let Some(f) = &limit.fetch else { return Ok(Box::new(Limited { rows, skip, left: None, ties: None })) };
    let ties = f.ties.clone().map(|keys| Ties { keys, last: None });
    if !f.percent {
        let left = count(&f.count, subqueries, &Rule::FETCH)?;
        return Ok(Box::new(Limited { rows, skip, left: Some(left), ties }));
    }
    let mut held = HeldRows::new(space);
    for row in rows {
        held.push(row?)?;
    }
    let left = percent::count(&f.count, held.len(), subqueries)?;
    Ok(Box::new(Limited { rows: Box::new(held.into_iter()), skip, left: Some(left), ties }))
}

/// What a count must be, and what it raises if it is not.
struct Rule {
    /// The least it may be.
    least: i128,
    /// The exception it raises below that, or null.
    state: SqlState,
    /// The rule broken, and what the count is.
    what: &'static str,
}

impl Rule {
    /// [GR6b] An `<offset row count>`.
    const OFFSET: Rule = Rule {
        least: 0,
        state: SqlState::INVALID_ROW_COUNT_IN_RESULT_OFFSET,
        what: "[ISO/IEC 9075-2:2023 §7.17 GR6b] the offset row count",
    };
    /// [GR7c] A `<fetch first row count>`.
    const FETCH: Rule = Rule {
        least: 1,
        state: SqlState::INVALID_ROW_COUNT_IN_FETCH_FIRST,
        what: "[ISO/IEC 9075-2:2023 §7.17 GR7c] the fetch first row count",
    };
}

/// The count `e`, evaluated with `subqueries`, as `rule` says it must be: a
/// number of rows, past `usize::MAX` that.
fn count(e: &Expr, subqueries: &dyn Subqueries, rule: &Rule) -> Result<usize> {
    let v = eval(e, &Vec::new(), subqueries)?;
    if v.is_null() {
        return fail_with(Code::Data, rule.state, format!("{} is null, not a number of rows", rule.what));
    }
    let Value::Exact(d) = v else { return Err(broken("a row count is not an exact number")) };
    if d.scale != 0 {
        return Err(broken("a row count is not of scale 0"));
    }
    let n = d.units;
    if n < rule.least {
        return fail_with(Code::Data, rule.state, format!("{} is {n}, less than {}", rule.what, rule.least));
    }
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

/// [GR7d i] Of WITH TIES, the keys of the rows' order, and the last row
/// kept by the count, once it has been.
struct Ties {
    keys: Vec<SortKey>,
    last: Option<Row>,
}

/// The rows of `rows` past the first `skip`, at most `left` of them if
/// given, and of `ties` the peers of the last of those after it; an error
/// passed on as it comes.
struct Limited {
    rows: RowIter,
    skip: usize,
    left: Option<usize>,
    ties: Option<Ties>,
}

impl Iterator for Limited {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if self.left == Some(0) {
                return self.peer();
            }
            let row = self.rows.next()?;
            let Ok(kept) = &row else { return Some(row) };
            if self.skip > 0 {
                self.skip = self.skip.saturating_sub(1);
                continue;
            }
            if let Some(left) = &mut self.left {
                *left = left.saturating_sub(1);
                if let (0, Some(ties)) = (*left, &mut self.ties) {
                    ties.last = Some(kept.clone());
                }
            }
            return Some(row);
        }
    }
}

impl Limited {
    /// Once the count is kept, the next row if it is a peer of the last
    /// kept [GR7d i]; else none, and no row more is read.
    fn peer(&mut self) -> Option<Result<Row>> {
        let ties = self.ties.as_mut()?;
        let last = ties.last.as_ref()?;
        let row = self.rows.next()?;
        let Ok(next) = &row else { return Some(row) };
        if compare_rows(&ties.keys, last, next) == Ordering::Equal {
            return Some(row);
        }
        self.ties = None;
        None
    }
}

// Unit tests: the rows kept of each count, each count refused, the peers
// of WITH TIES and a percentage's count (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
