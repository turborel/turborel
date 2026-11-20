//! \[DESIGN\] (RULES.md D85) The conjunct an index scan decides of each
//! entry (`Plan::IndexScan::decided`): of an entry that is one point, not
//! the origin an empty geometry projects to, by the call's function from
//! the point (`ScalarFunction::point_compared`), its row read only if a
//! column is read of it, or a reader may not see it
//! (`TableStorage::visible_to_all`); of any other, by the call on its row,
//! as a filter would.

use std::sync::Arc;

use super::super::entries::{Reading, fetch, fetch_decided};
use super::super::{Runner, eval_cond};
use crate::catalog::{KeyShape, QualifiedName};
use crate::db::State;
use crate::error::Result;
use crate::functions::ScalarRef;
use crate::geometry::fixed_srid;
use crate::index::Key;
use crate::logical::{Cond, Expr, Truth};
use crate::physical::Decided;
use crate::storage::{Row, RowId, TableStorage};
use crate::txn::TxId;
use crate::value::{CompOp, Value};

/// A scan's decision of its entries.
pub(super) struct Deciding {
    cond: Cond,
    /// The call a point's coordinates decide, if the conjunct is one.
    point: Option<Call>,
    /// Whether no column is read of a row decided true, so the row is
    /// made of nulls, if every reader sees it: as wide as the table where
    /// each row is followed by its id, else of no column.
    unread: bool,
    width: Option<usize>,
    runner: Runner,
}

/// "`f(column, other) op value`", or the column second: its function, the
/// constant it is given, the comparison, and the column's reference.
struct Call {
    function: ScalarRef,
    other: Value,
    op: CompOp,
    value: Value,
    srid: i32,
}

impl Deciding {
    /// The decision of the conjunct `decided` of a scan of `table`
    /// reading no column if `unread`, each row followed by its id if
    /// `located`.
    pub(super) fn new(decided: Decided, db: &Arc<State>, tx: TxId, table: &QualifiedName, (unread, located): (bool, bool)) -> Deciding {
        let desc = db.catalog.table(table);
        let width = desc.map(|t| if located { t.columns.len() } else { 0 });
        let srid = desc.and_then(|t| t.columns.get(decided.column)).and_then(|c| fixed_srid(&c.data_type));
        let point = srid.and_then(|srid| call_of(&decided.cond, decided.column, srid));
        Deciding { cond: decided.cond, point, unread, width, runner: Runner::new(db, tx) }
    }

    /// The row of the entry `key` of row `id`, read of `storage` by
    /// `reading`, if the conjunct holds of it: of a point entry decided
    /// true, read only if a column is, or a reader may not see the row;
    /// of any other entry, read and judged.
    pub(super) fn row(
        &self,
        (storage, ix): (&dyn TableStorage, &KeyShape),
        reading: &mut Reading,
        (tx, id): (TxId, RowId),
        key: &Key,
    ) -> Result<Option<Row>> {
        match self.of_entry(key) {
            Some(Err(error)) => Err(error),
            Some(Ok(Some(true))) => {
                if let Some(width) = self.width.filter(|_| self.unread && reading.visible_to_all(storage, id)) {
                    return Ok(Some(vec![Value::Null; width]));
                }
                Ok(fetch_decided(storage, ix, reading, (tx, id), key)?.map(|(_, row)| row))
            }
            Some(Ok(Some(false) | None)) => Ok(None),
            None => {
                let Some((_, row)) = fetch(storage, ix, reading, (tx, id), key)? else { return Ok(None) };
                Ok((eval_cond(&self.cond, &row, &self.runner)? == Truth::True).then_some(row))
            }
        }
    }

    /// The conjunct of the entry `key`, if the entry is one point the
    /// call's function decides (`ScalarFunction::point_compared`).
    fn of_entry(&self, key: &Key) -> Option<Result<Option<bool>>> {
        let call = self.point.as_ref()?;
        let [Value::Approx(x0), Value::Approx(y0), Value::Approx(x1), Value::Approx(y1)] = key.0.as_slice() else { return None };
        let single = x0.to_bits() == x1.to_bits() && y0.to_bits() == y1.to_bits();
        let origin = x0.abs() <= 0.0_f64 && y0.abs() <= 0.0_f64;
        if !single || origin {
            return None;
        }
        call.function.0.point_compared(([*x0, *y0], call.srid), &call.other, call.op, &call.value)
    }
}

/// The call of `cond`, "`f(column, other) op value`", the call or the
/// column second, if it is one of column `column`, of reference `srid`.
fn call_of(cond: &Cond, column: usize, srid: i32) -> Option<Call> {
    let Cond::Compare(left, op, right) = cond else { return None };
    let ((function, args), op, value) = if let (Expr::Call(function, args), Expr::Const(value)) = (left, right) {
        ((function, args), *op, value)
    } else if let (Expr::Const(value), Expr::Call(function, args)) = (left, right) {
        ((function, args), op.flip(), value)
    } else {
        return None;
    };
    let (at, other) = match args.as_slice() {
        [Expr::Column(at), Expr::Const(other)] | [Expr::Const(other), Expr::Column(at)] => (*at, other),
        _ => return None,
    };
    (at == column).then(|| Call { function: function.clone(), other: other.clone(), op, value: value.clone(), srid })
}
