//! [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] OFFSET and FETCH FIRST run: the
//! counts evaluated once, as the rows are first asked for, then the first
//! rows of the input passed over and the rest kept up to the count, in
//! their order. No row past those kept is read, so an input that gives
//! its rows as it reads them, as a scan in an index's order does, reads
//! no more.

use std::sync::Arc;

use super::subquery::{Runner, Subqueries};
use super::{RowIter, broken, eval, failed};
use crate::db::State;
use crate::error::{Code, Result, SqlState, fail_with};
use crate::logical::{Expr, Limit};
use crate::physical::Plan;
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Value;

/// The rows the limit `plan` keeps.
#[inline(never)]
pub(super) fn limited(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Limit { limit, input } = plan else { return failed(broken("a limit is not one")) };
    match counts(&limit, &Runner::new(db, tx)) {
        Ok((skip, left)) => Box::new(Limited { rows: super::execute(*input, db, tx), skip, left }),
        Err(e) => failed(e),
    }
}

/// The rows to pass over and the most to keep, none if every one is,
/// of `limit`, its counts evaluated with `subqueries`.
///
/// # Errors
///
/// SQLSTATE 2201X if the offset is below 0 [GR6b], 2201W if the count of
/// FETCH FIRST is below 1 [GR7c], either null as well, being no number of
/// rows; their evaluation's.
fn counts(limit: &Limit, subqueries: &dyn Subqueries) -> Result<(usize, Option<usize>)> {
    let offset = match &limit.offset {
        Some(e) => count(e, subqueries, &Rule::OFFSET)?,
        None => 0,
    };
    let fetch = limit.fetch.as_ref().map(|e| count(e, subqueries, &Rule::FETCH)).transpose()?;
    Ok((offset, fetch))
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

/// The rows of `rows` past the first `skip`, at most `left` of them if
/// given; an error passed on as it comes.
struct Limited {
    rows: RowIter,
    skip: usize,
    left: Option<usize>,
}

impl Iterator for Limited {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if self.left == Some(0) {
                return None;
            }
            let row = self.rows.next()?;
            if row.is_ok() && self.skip > 0 {
                self.skip = self.skip.saturating_sub(1);
                continue;
            }
            if let (Some(left), true) = (&mut self.left, row.is_ok()) {
                *left = left.saturating_sub(1);
            }
            return Some(row);
        }
    }
}

// Unit tests: the rows kept of each count, and each count refused
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
