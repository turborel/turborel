//! [ISO/IEC 9075-2:2023 §4.46.2, §15.21 GR5a] The rows of a transition
//! table: the old or new rows of a state change, which a triggered action
//! reads by the old or new transition table name its trigger gives.
//!
//! The action is bound once for its trigger, its plans holding a handle
//! to the rows (`TransitionRows`), and given the rows of each state change
//! it is invoked for before it runs. They are a list of held rows
//! (`exec/held.rs`), spilled past MEMORY.WORK_BYTES, read only while the
//! statement is not cancelled (RULES.md D44).

use std::fmt;
use std::sync::{Arc, Mutex};

use super::{HeldRows, RowIter, broken, failed};
use crate::db::State;
use crate::error::{Result, SqlError};
use crate::physical::Plan;
use crate::spill::ListSpace;
use crate::storage::Row;
use crate::txn::{TxId, checked};

/// The rows of a transition table, shared by every plan of the triggered
/// action that reads it, and by the state change that gives them.
#[derive(Clone)]
pub struct TransitionRows(Arc<Mutex<HeldRows<Row>>>);

impl fmt::Debug for TransitionRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TransitionRows")
    }
}

impl TransitionRows {
    /// A transition table of no rows yet, held in `space`.
    pub fn new(space: ListSpace) -> TransitionRows {
        TransitionRows(Arc::new(Mutex::new(HeldRows::new(space))))
    }

    /// A transition table of `rows`.
    pub fn with(rows: HeldRows<Row>) -> TransitionRows {
        TransitionRows(Arc::new(Mutex::new(rows)))
    }

    /// Makes `rows` its rows, in place of those it had: the state change's
    /// the triggered action is invoked for next.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its lock was poisoned.
    pub fn give(&self, rows: HeldRows<Row>) -> Result<()> {
        *self.0.lock().map_err(|_| poisoned())? = rows;
        Ok(())
    }

    /// Its row at `place`, if it has one so far along.
    fn get(&self, place: usize) -> Result<Option<Row>> {
        self.0.lock().map_err(|_| poisoned())?.get(place)
    }
}

/// A transition table's lock held by a thread that failed.
fn poisoned() -> SqlError {
    broken("a transition table's lock was poisoned")
}

/// The rows of the transition table `plan`, each read while the statement
/// of `tx` is not cancelled.
pub(super) fn transition(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Transition { rows } = plan else { return failed(broken("a transition table's read is not one")) };
    let mut place = 0_usize;
    let read = std::iter::from_fn(move || {
        let row = rows.get(place);
        place = place.saturating_add(1);
        row.transpose()
    });
    Box::new(checked(read, db.tx.stop_of(tx)))
}
