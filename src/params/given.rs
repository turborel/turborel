//! \[EXTENSION\] Rows given an INSERT of one row of dynamic parameters
//! (RULES.md D530): each the values of its parameters, as an execution
//! of it takes them, so one execution inserts them all, together
//! (`db/write/given.rs`), as a bulk copy does.

use std::sync::Arc;

use super::Params;
use crate::storage::Row;

impl Params {
    /// These parameters, giving `rows`.
    pub(crate) fn giving(mut self, rows: Vec<Row>) -> Params {
        self.given = Some(Arc::new(rows));
        self
    }

    /// The rows given, if any.
    pub(crate) fn given(&self) -> Option<&Arc<Vec<Row>>> {
        self.given.as_ref()
    }
}
