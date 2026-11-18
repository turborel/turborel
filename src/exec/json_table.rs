//! [ISO/IEC 9075-2:2023 §7.11] A `<JSON table>`, run: of each row of its
//! input, the table references before it, that row and each row of the
//! JSON table of its operands' values (`JsonTable::open`). Of the second
//! operand of a LEFT join, an input row none of whose rows its condition
//! makes true is kept, its own columns null [§7.10 GR1d], as a collection
//! derived table's is (`exec/unnest.rs`).
//!
//! The rows stream: a JSON table's rows are made one at a time as they
//! are read, a DEFAULT evaluated only when a column takes its value, and
//! each is taken only while the statement is not cancelled (RULES.md
//! D44). A cast that cut a string warns the statement, 01004.

use std::sync::Arc;

use super::{RowIter, Runner, broken, eval, eval_cond, execute, failed, warn};
use crate::db::State;
use crate::error::Result;
use crate::json::{Cursor, JsonTable};
use crate::logical::{Truth, Unnest};
use crate::physical::Plan;
use crate::storage::Row;
use crate::txn::{TxId, checked};
use crate::value::Value;

/// The rows of `plan`, a JSON table over its input.
#[inline(never)]
pub(super) fn json_table(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Unnest(u) = plan else { return failed(broken("a JSON table is not one")) };
    let (u, input) = u.split();
    let Some(table) = u.json.clone() else { return failed(broken("a JSON table has no table")) };
    let rows = Rows { input: execute(input, db, tx), runner: Runner::new(db, tx), u, table, current: None };
    Box::new(checked(rows, db.tx.stop_of(tx)))
}

/// The rows of a JSON table over its input.
struct Rows {
    input: RowIter,
    runner: Runner,
    u: Unnest<()>,
    table: Arc<JsonTable>,
    /// The input row being read, the JSON table's rows of it left, and
    /// whether one of them was kept.
    current: Option<(Row, Cursor, bool)>,
}

impl Iterator for Rows {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if self.current.is_some() {
                if let Some(made) = self.made() {
                    return Some(made);
                }
                // [§7.10 GR1d] of a LEFT join, the input row null
                // extended if none of its rows was kept.
                if let Some((row, _, false)) = self.current.take()
                    && self.u.outer.is_some()
                {
                    let mut padded = row;
                    padded.resize(padded.len().saturating_add(self.table.width), Value::Null);
                    return Some(Ok(padded));
                }
            }
            let row = match self.input.next()? {
                Ok(row) => row,
                Err(e) => return Some(Err(e)),
            };
            match self.opened(&row) {
                Ok(cursor) => self.current = Some((row, cursor, false)),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

impl Rows {
    /// The JSON table of the input row `row`: of the values of its context
    /// item and PASSING arguments there.
    fn opened(&self, row: &Row) -> Result<Cursor> {
        let mut operands = self.u.collections.iter();
        let context = operands.next().map_or(Ok(Value::Null), |e| eval(e, row, &self.runner))?;
        let passing = operands.take(self.table.passing.len()).map(|e| eval(e, row, &self.runner)).collect::<Result<Vec<_>>>()?;
        self.table.open(&context, &passing)
    }

    /// The next row of the current input row, its condition true if it is
    /// of a LEFT join; none after its last.
    fn made(&mut self) -> Option<Result<Row>> {
        let Rows { runner, u, table, current, .. } = self;
        let (row, cursor, kept) = current.as_mut()?;
        let first_default = table.passing.len().saturating_add(1);
        let mut defaults = |k: usize| {
            let e = u.collections.get(first_default.saturating_add(k)).ok_or_else(|| broken("a JSON table lost a DEFAULT"))?;
            eval(e, row, runner)
        };
        loop {
            let (values, warning) = match cursor.next(table, &mut defaults)? {
                Ok(made) => made,
                Err(e) => return Some(Err(e)),
            };
            if let Some(state) = warning {
                warn(runner, state);
            }
            let mut made = row.clone();
            made.extend(values);
            let Some(cond) = &u.outer else { return Some(Ok(made)) };
            match eval_cond(cond, &made, runner) {
                Ok(Truth::True) => {
                    *kept = true;
                    return Some(Ok(made));
                }
                Ok(Truth::False | Truth::Unknown) => {}
                Err(e) => return Some(Err(e)),
            }
        }
    }
}
