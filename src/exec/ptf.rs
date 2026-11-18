//! [ISO/IEC 9075-2:2023 §7.6 GR4, §9.25] The rows of a `<PTF derived
//! table>`: its arguments' values found, its table arguments' rows read
//! whole, and its invocation run on them (`db/ptf/execute.rs`) as the
//! cursor opens.

use std::sync::Arc;

use super::{RowIter, Runner, broken, eval, execute, failed};
use crate::db::State;
use crate::error::Result;
use crate::physical::Plan;
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Value;

/// The rows of the `<PTF derived table>` `plan`.
pub(super) fn polymorphic(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Polymorphic(p) = plan else { return failed(broken("a <PTF derived table> is not one")) };
    let runner = Runner::new(db, tx);
    let found = || -> Result<Vec<Row>> {
        let empty = Vec::new();
        let arguments: Vec<Value> = p.arguments.iter().map(|e| eval(e, &empty, &runner)).collect::<Result<_>>()?;
        let mut rows = Vec::with_capacity(p.inputs.len());
        for input in p.inputs {
            rows.push(execute(input, db, tx).collect::<Result<Vec<Row>>>()?);
        }
        p.call.run(arguments, &rows, &runner, (db, tx))
    };
    match found() {
        Ok(rows) => Box::new(rows.into_iter().map(Ok)),
        Err(e) => failed(e),
    }
}
