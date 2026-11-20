//! \[DESIGN\] (RULES.md D490) The batches a groupjoin's probe operand is
//! read in: of a subtree that gives them, its stream's, while
//! EXECUTOR.BATCHES is on; else its rows gathered into batches of every
//! column, so the groupjoin feeds its set functions by batch either way.

use std::sync::Arc;

use super::Batches;
use super::joined::gathered;
use super::stream::{self, layout};
use crate::db::State;
use crate::error::Result;
use crate::exec::{batched, execute};
use crate::physical::Plan;
use crate::txn::TxId;

/// The batches of `plan`, whose rows are `width` columns wide, and the
/// column of its rows each vector of a batch holds.
///
/// # Errors
///
/// As opening the stream fails.
pub(in crate::exec) fn probed(plan: Plan, width: usize, (db, tx): (&Arc<State>, TxId)) -> Result<(Batches, Vec<usize>)> {
    if batched(db) && layout(&plan, db).is_some() {
        let opened = stream::open(plan, db, tx)?;
        return Ok((opened.batches, opened.columns));
    }
    let columns: Vec<usize> = (0..width).collect();
    Ok((gathered(execute(plan, db, tx), &columns), columns))
}

/// The column of `plan`'s rows, `width` wide, each vector of its batches
/// holds, as `probed` reads them.
pub(in crate::exec) fn probe_layout(plan: &Plan, width: usize, db: &State) -> Vec<usize> {
    if batched(db)
        && let Some((columns, _)) = layout(plan, db)
    {
        return columns;
    }
    (0..width).collect()
}
