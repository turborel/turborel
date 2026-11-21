//! \[EXTENSION\] Rows given an INSERT of one row of dynamic parameters
//! (RULES.md D530): each the values of its parameters, as an execution
//! of it takes them, so one execution inserts them all, together
//! (`db/write/given.rs`), as a bulk copy does.

use std::sync::Arc;

use super::{ParameterType, Params};
use crate::error::{Code, Result, SqlError};
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

/// `row`, the values of parameters of types `types`, each cast to its type
/// as an execution takes it [ISO/IEC 9075:1992 §17.9 GR5f].
///
/// # Errors
///
/// CAST's; SQLCODE -910 for a cast that warns, which an execution of its
/// own would tell, so the row is executed alone.
pub(crate) fn conformed_row(row: Row, types: &[ParameterType]) -> Result<Row> {
    row.into_iter()
        .zip(types)
        .map(|(value, t)| match super::conformed(value, &t.clone().data_type())? {
            (value, None) => Ok(value),
            (_, Some(_)) => Err(SqlError::new(Code::Api, "[DESIGN] a value given with others warns as it is cast")),
        })
        .collect()
}
