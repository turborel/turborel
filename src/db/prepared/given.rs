//! \[EXTENSION\] A prepared INSERT of one row of dynamic parameters
//! executed once for many rows, each the values of its parameters
//! (RULES.md D530): what a bulk copy does with each batch of its rows.

use super::super::area::commands;
use super::super::{Completion, Session};
use super::{Kind, Prepared};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::params::{Params, conformed_row};
use crate::storage::Row;
use crate::value::DataType;

impl Session {
    /// \[EXTENSION\] Executes `p`, an INSERT of one row of dynamic
    /// parameters, once for all of `rows`, each the values of its
    /// parameters: as many executions of it would, one a row, as one
    /// statement, which fails whole if any row does.
    ///
    /// # Errors
    ///
    /// SQLSTATE 07001 if a row is not one value for each parameter [ISO/IEC
    /// 9075:1992 §17.9 GR5a]; SQLCODE -910 if `p` is not such an INSERT;
    /// as `execute_prepared`.
    pub fn execute_prepared_rows(&self, p: &Prepared, rows: &[Row]) -> Result<Completion> {
        if p.kind != Kind::Statement {
            return Err(super::super::write::not_given());
        }
        let types: Vec<DataType> = p.parameters.iter().map(|t| t.clone().data_type()).collect();
        let mut given = Vec::with_capacity(rows.len());
        for row in rows {
            Self::one_each(p, row.len())?;
            // [ISO/IEC 9075-2:2023 §4.37.5] a large object sent by its
            // locator is the value it names.
            given.push(conformed_row(self.unlocated(row)?, &types)?);
        }
        let Some(first) = given.first().cloned() else {
            let msg = "[DESIGN] an execution for many rows is given at least one";
            return Err(SqlError::new(Code::Dynamic, msg).with_state(SqlState::USING_CLAUSE_MISMATCH));
        };
        let mut params = Params::executing(p.outputs.expanded(first)).implying(p.implicit.clone()).giving(given);
        self.inner.borrow_mut().dynamic.positioned = None;
        self.as_prepared(p, || self.execute_as(&p.sql, &mut params, Some((commands::EXECUTE, &p.plans))))
    }
}
