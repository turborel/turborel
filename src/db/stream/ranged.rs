//! \[DESIGN\] A cursor's rows read direct as it opens (RULES.md D931). A
//! plan kept that is a direct range read (`exec/range.rs`) is read where
//! it is kept, its rows held for the fetches, with no operator built.
//!
//! OPEN creates the cursor's table, and reading it later must give the
//! same rows and fail as the plan would (`stream.rs`). A plan that can
//! raise an exception of a General Rule is read in full at OPEN, which
//! raises it, and so is one read direct. A plan that sorts or groups
//! reads every row before it gives one, and so does its direct read. Any
//! other gives its rows as they are fetched, so a cursor closed early
//! reads few: its range is read direct only while it is short, no more
//! than `MOST_AHEAD` rows, and a longer one is left to the executor, which
//! reads it as it is fetched, as are the plan's ranges after it. A plan that cannot raise fails only as its
//! range is read, at the first fetch, so one whose direct read fails is
//! left to the executor too, which fails there.
//!
//! Rows read at OPEN are those a settled cursor holds (`Stream::settle`),
//! so a change its transaction makes after is not seen, as before.

use std::sync::Arc;

use super::super::State;
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::exec::{Db, HeldIter, HeldRows, RangeAt, Runner, Slots};
use crate::physical::Kept;
use crate::storage::Row;
use crate::txn::Txn;

/// \[DESIGN\] The most rows of a range that gives its rows as they are
/// fetched read ahead as its cursor opens: the rows of a few leaves of an
/// index, which a cursor fetched to its end reads anyway, and little
/// beside the descent of one closed early. A longer range is read by the
/// executor, as it is fetched.
const MOST_AHEAD: usize = 128;

/// The rows of `kept` read direct for the transaction `tx`, with the
/// values `slots`, the tables `reads` locked first: none if it is no
/// direct range read, or is left to the executor.
///
/// # Errors
///
/// The plan's, of one that can raise; a lock's; and holding the rows'.
pub(super) fn read(
    st: &Arc<State>,
    tx: &Txn,
    kept: &Kept,
    (reads, slots): (&Arc<[QualifiedName]>, Option<&Slots>),
) -> Result<Option<HeldIter<Row>>> {
    let Some(range) = kept.range(st) else { return Ok(None) };
    for t in reads.iter() {
        st.read_lock(tx, t)?;
    }
    let db = slots.map_or_else(|| Db::of(st), |slots| Db::slotted(st, Arc::clone(slots)));
    let runner = Runner::new(&db, tx.id);
    let at = RangeAt { st, tx: tx.id, values: slots.map_or(&[], |slots| &**slots), subqueries: &runner };
    let whole = range.raises() || range.holds();
    let most = (!whole).then(|| MOST_AHEAD.saturating_add(1));
    let read = range.rows(&at, most);
    if range.raises() && read.is_err() {
        return read.map(|_| None);
    }
    // A plan that cannot raise fails as it is fetched from.
    let Ok(Some(rows)) = read else { return Ok(None) };
    if most.is_some_and(|most| rows.len() >= most) {
        // A range this long once is the executor's from now on.
        range.outgrown();
        return Ok(None);
    }
    let mut held = HeldRows::new(st.list_space());
    for row in rows {
        held.push(row)?;
    }
    Ok(Some(held.into_iter()))
}
