//! [ISO/IEC 9075-2:2023 §7.6 SR8] A `<collection derived table>`, run:
//! of each row of its input, the rows its collections' elements make, one
//! for each ordinal from 1 to the greatest cardinality among them, an
//! element past a collection's cardinality null [SR8j, SR8k]. A null
//! collection has none [SR8f]. Of the second operand of a LEFT join, an
//! input row none of whose rows its condition makes true is kept, its
//! own columns null [§7.10 GR1d].
//!
//! The rows of one input row are made together, at most
//! `MAX_ARRAY_CARDINALITY` of them, and each is taken only while the
//! statement is not cancelled (RULES.md D44).

use std::sync::Arc;

use super::{RowIter, Runner, broken, eval, eval_cond, execute, failed};
use crate::db::State;
use crate::error::Result;
use crate::logical::{Truth, Unnest};
use crate::physical::Plan;
use crate::storage::Row;
use crate::txn::{TxId, checked};
use crate::value::{Decimal, Value};

/// The rows of `plan`, a collection derived table over its input.
#[inline(never)]
pub(super) fn unnested(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Unnest(u) = plan else { return failed(broken("a collection derived table is not one")) };
    let (u, input) = u.split();
    let (rows, runner) = (execute(input, db, tx), Runner::new(db, tx));
    let made = rows.flat_map(move |read| {
        let found = read.and_then(|row| of_row(&u, row, &runner));
        let (made_rows, error) = match found {
            Ok(of_input) => (of_input, None),
            Err(e) => (Vec::new(), Some(Err(e))),
        };
        made_rows.into_iter().map(Ok).chain(error)
    });
    Box::new(checked(made, db.tx.stop_of(tx)))
}

/// The rows `u` makes of the input row `row`: `row`, then its elements of
/// each ordinal, then the ordinal if it is asked for.
fn of_row(u: &Unnest<()>, row: Row, runner: &Runner) -> Result<Vec<Row>> {
    let collections = u.collections.iter().map(|e| collection(eval(e, &row, runner)?)).collect::<Result<Vec<_>>>()?;
    // [SR8f to SR8h] MAXCARD, a null collection's cardinality 0.
    let most = collections.iter().map(|c| c.as_ref().map_or(0, |a| a.len())).max().unwrap_or(0);
    let mut out = Vec::with_capacity(most);
    for n in 0..most {
        let mut made = Vec::with_capacity(row.len().saturating_add(u.width()));
        made.extend(row.iter().cloned());
        for (c, spread) in collections.iter().zip(&u.spread) {
            // [SR8j, SR8k] the element of ordinal n + 1, else null.
            let element = c.as_ref().and_then(|a| a.get(n)).cloned().unwrap_or(Value::Null);
            spread_into(&mut made, element, *spread)?;
        }
        if u.ordinality {
            made.push(Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX).saturating_add(1), 0)));
        }
        out.push(made);
    }
    let Some(cond) = &u.outer else { return Ok(out) };
    let mut kept = Vec::with_capacity(out.len());
    for made in out {
        if eval_cond(cond, &made, runner)? == Truth::True {
            kept.push(made);
        }
    }
    if kept.is_empty() {
        let mut padded = row;
        padded.resize(padded.len().saturating_add(u.width()), Value::Null);
        kept.push(padded);
    }
    Ok(kept)
}

/// The elements of a collection's value `v`, none if it is null.
fn collection(v: Value) -> Result<Option<Arc<[Value]>>> {
    match v {
        Value::Array(a) => Ok(Some(a)),
        Value::Null => Ok(None),
        Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Row(_)
        | Value::Extension(_) => Err(broken("a collection value expression is not an array")),
    }
}

/// Adds `element` to `made`: itself, or [SR8n] each of its `degree`
/// fields if its type is a row type, all null if it is null.
fn spread_into(made: &mut Row, element: Value, degree: Option<usize>) -> Result<()> {
    let Some(degree) = degree else {
        made.push(element);
        return Ok(());
    };
    if let Value::Row(fields) = &element
        && fields.len() == degree
    {
        made.extend(fields.iter().cloned());
    } else if element.is_null() {
        made.resize(made.len().saturating_add(degree), Value::Null);
    } else {
        return Err(broken("an element of a row type is not a row of its degree"));
    }
    Ok(())
}
