//! \[DESIGN\] The object row of a kept UPDATE or DELETE whose plan is a
//! point read, read without the executor's operators (RULES.md D752).

use std::sync::Arc;

use super::super::State;
use crate::binder::BoundWrite;
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::exec::{Db, HeldRows, row_entries};
use crate::physical::Kept;
use crate::storage::{Row, RowId};
use crate::txn::Txn;
use crate::value::Value;

/// The object rows of `bound`, whose plan kept `kept` reads `reads`, run
/// in `db` with `values`, after taking their locks as `object_rows` does:
/// the row its point read finds, if one, held as `exec::row_entries`
/// holds a plan's rows; or the plan's rows, once point operations are
/// off.
///
/// # Errors
///
/// A lock is not granted; as `PointRead::entry`, `Kept::opened` or
/// `row_entries`.
pub(super) fn object_row(
    st: &Arc<State>,
    tx: &Txn,
    (bound, kept, reads): (&BoundWrite, &Kept, &[QualifiedName]),
    (db, values): (&Db, &[Value]),
) -> Result<HeldRows<(RowId, Row)>> {
    for t in reads.iter().cloned().chain(bound.set_tables()) {
        st.read_lock(tx, &t)?;
    }
    let Some(point) = kept.point(st) else {
        let (plan, _) = kept.opened(Some(values.into()), crate::optimizer::prepared::budget(st))?;
        return row_entries(plan, db, tx.id);
    };
    let mut held = HeldRows::new(st.list_space());
    if let Some(row) = point.entry(st, tx.id, values)? {
        held.push(row)?;
    }
    Ok(held)
}
