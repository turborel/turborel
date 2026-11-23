//! The searched DELETE's rows deleted [X3.135-1986 §8.5], as `update_rows`
//! updates a searched UPDATE's.

use super::{Written, object_rows, portion};
use crate::db::StatementContext;
use crate::db::integrity::Effects;
use crate::error::Result;
use crate::exec::Runner;
use crate::storage::Row;
use crate::txn::Again;

/// [X3.135-1986 §8.5] the searched DELETE bound as `w`: the rows it
/// deleted [ISO/IEC 9075:1992 §18.1 GR1e], none for no data [X3.135-1986
/// §7.3 GR3a iv].
pub(in crate::db) fn delete_searched(cx: StatementContext<'_>, w: Written, mut effects: Effects<'_>) -> Result<usize> {
    match delete_rows(cx, w, &mut effects)? {
        Some(n) => {
            cx.st.end_statement(cx.tx, effects)?;
            Ok(n)
        }
        None => Ok(0),
    }
}

/// `delete_searched`'s rows deleted, as `update_rows` updates them, the
/// statement not yet ended.
pub(in crate::db) fn delete_rows(cx: StatementContext<'_>, w: Written, effects: &mut Effects<'_>) -> Result<Option<usize>> {
    let StatementContext { st, tx } = cx;
    let (w, mut objects) = object_rows(st, tx, w)?;
    if objects.is_empty() && effects.triggered.is_none() {
        return Ok(None);
    }
    // [ISO/IEC 9075-2:2023 §15.8 GR2] its BEFORE triggers first.
    effects.before_delete(cx, &w.bound.table, &mut objects)?;
    let runner = Runner::new(&w.db(st), tx.id);
    let mut leftovers = portion::Leftovers::of(st, &w)?;
    let mut count = 0_usize;
    // What its writes need of its table, found once (RULES.md D872).
    let writing = st.writing(&w.bound.table)?;
    for object in objects {
        let (id, _) = object?;
        // A row another transaction changed is deleted in the version it
        // committed, if it is still one to delete.
        let mut recheck = |newest: &Row| Ok(if w.holds_of(newest, &runner)? { Again::Delete } else { Again::Skip });
        if let Some(old) = st.rewrite_row_in(tx, (&writing, id), None, &mut recheck)? {
            leftovers.keep(st, &w, &old, &runner)?;
            effects.deleted(&w.bound.table, id, old)?;
            count = count.saturating_add(1);
        }
    }
    leftovers.insert(cx, &w, effects)?;
    Ok(Some(count))
}
