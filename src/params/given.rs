//! \[EXTENSION\] Rows given an INSERT of one row of dynamic parameters
//! (RULES.md D530): each the values of its parameters, as an execution
//! of it takes them, so one execution inserts them all, together
//! (`db/write/given.rs`), as a bulk copy does.

use std::sync::{Arc, Mutex};

use super::Params;
use crate::error::{Code, Result, SqlError};
use crate::storage::Row;
use crate::sync::lock;
use crate::value::DataType;

/// Rows given, held until the execution that inserts them takes them, so
/// none is copied for it (RULES.md D531).
pub(crate) type Given = Arc<Mutex<Option<Vec<Row>>>>;

/// \[EXTENSION\] Of an execution given many rows, what part of a load it
/// is (RULES.md D532): a load of its own, or one of a load's executions,
/// more to come or the last, as a bulk copy's are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Load {
    /// A load of its own, of its rows alone.
    Alone,
    /// One of a load's executions, more to come.
    Going,
    /// A load's last execution, which ends it.
    Ending,
}

impl Params {
    /// These parameters, giving `rows`, as part `load` of a load.
    pub(crate) fn giving(mut self, rows: Vec<Row>, load: Load) -> Params {
        self.given = Some((Arc::new(Mutex::new(Some(rows))), load));
        self
    }

    /// The rows given, if any.
    pub(crate) fn given(&self) -> Option<&Given> {
        self.given.as_ref().map(|(given, _)| given)
    }

    /// What part of a load the execution given rows is, if it is given
    /// them.
    pub(crate) fn load(&self) -> Option<Load> {
        self.given.as_ref().map(|&(_, load)| load)
    }
}

/// The rows of `given`, taken from it.
///
/// # Errors
///
/// SQLCODE -910 if they were taken before, or its lock is poisoned.
pub(crate) fn taken(given: &Given) -> Result<Vec<Row>> {
    lock(given)?.take().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the rows given an execution are inserted by it once"))
}

/// `row`, the values of parameters of types `types`, each cast to its type
/// as an execution takes it [ISO/IEC 9075:1992 §17.9 GR5f].
///
/// # Errors
///
/// CAST's; SQLCODE -910 for a cast that warns, which an execution of its
/// own would tell, so the row is executed alone.
pub(crate) fn conformed_row(row: Row, types: &[DataType]) -> Result<Row> {
    row.into_iter()
        .zip(types)
        .map(|(value, t)| match super::conformed(value, t)? {
            (value, None) => Ok(value),
            (_, Some(_)) => Err(SqlError::new(Code::Api, "[DESIGN] a value given with others warns as it is cast")),
        })
        .collect()
}
