//! \[DESIGN\] (RULES.md D668) A leaf of a chain once a phase of predicate
//! transfer read it: its batches held, each with the rows the phase's
//! filters kept of it, read once in the leaf's place; or, a leaf the phase
//! could not hold within its work area, or did not read, its plan, read
//! then with every filter the phase made for it told it, as a hash join
//! tells its scan its runtime filters (`batch/runtime.rs`).

use std::fmt;
use std::sync::{Arc, Mutex};

use super::super::late::{Deferred, resolved};
use super::super::runtime::Told;
use super::super::stream::{self, Batches, Kept};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::broken;
use crate::physical::Plan;
use crate::txn::TxId;

/// \[DESIGN\] A leaf a phase of predicate transfer read (RULES.md D668),
/// shared by the plans that hold it, as a plan is cloned.
#[derive(Clone)]
pub struct HeldLeaf(Arc<Inner>);

/// What a held leaf gives, and the column of its rows each vector of its
/// batches holds, of rows of its width.
struct Inner {
    columns: Vec<usize>,
    width: usize,
    rows: Mutex<Option<Rows>>,
}

/// A leaf's rows: its batches held, or its plan and the filters it is told.
enum Rows {
    Held(Vec<Kept>),
    Told(Box<Plan>, Vec<Told>),
}

impl fmt::Debug for HeldLeaf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HeldLeaf")
    }
}

impl HeldLeaf {
    /// A leaf of the layout `(columns, width)` whose batches `kept` hold.
    pub(in crate::exec) fn held((columns, width): (Vec<usize>, usize), kept: Vec<Kept>) -> HeldLeaf {
        HeldLeaf(Arc::new(Inner { columns, width, rows: Mutex::new(Some(Rows::Held(kept))) }))
    }

    /// A leaf of the layout `(columns, width)`, read by `plan`, told `told`.
    pub(in crate::exec) fn told((columns, width): (Vec<usize>, usize), plan: Plan, told: Vec<Told>) -> HeldLeaf {
        HeldLeaf(Arc::new(Inner { columns, width, rows: Mutex::new(Some(Rows::Told(Box::new(plan), told))) }))
    }

    /// The columns of its rows its batches hold, and their width.
    pub(in crate::exec) fn layout(&self) -> (Vec<usize>, usize) {
        (self.0.columns.clone(), self.0.width)
    }

    /// Its batches, given once: those held, or those of its plan, told its
    /// filters, its columns decoded of the rows they keep alone.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was read before or its lock was poisoned; its
    /// plan's errors.
    pub(in crate::exec) fn batches(&self, (db, tx): (&Db, TxId)) -> Result<Batches> {
        let rows = self.0.rows.lock().map_err(|_| broken("a held leaf's lock was poisoned"))?.take();
        match rows.ok_or_else(|| broken("a held leaf was read twice"))? {
            Rows::Held(kept) => Ok(Box::new(kept.into_iter().map(Ok))),
            Rows::Told(plan, told) => {
                let width = self.0.width;
                Ok(resolved(stream::batches_with(*plan, (db, tx), &Deferred::all(width).with_told(told))?, crate::db::kernels::chosen(db)))
            }
        }
    }
}
