//! [ISO/IEC 9075-2:2023 §4.35.4, §9.24, §9.25, §9.27] Polymorphic table
//! functions as they run: the PTF data area of an invocation, compiled as
//! it is bound (`compile.rs`), its describe component procedure run then;
//! and its execution as the statement runs (`execute.rs`), its start,
//! fulfill and finish component procedures run on each virtual processor
//! in turn, the rows they pipe its result.
//!
//! A component procedure runs where a query evaluates it, as a function's
//! body does (`routines/program/executor.rs`), as none modifies SQL-data
//! [§11.60 SR10j i]. The statements it alone may run, on the names of the
//! PTF data area, are `statements.rs`'s.

mod area;
mod compile;
mod compiling;
mod execute;
mod statements;

pub(crate) use area::DataArea;
pub(crate) use compile::{Compiled, DescriptorColumns, Procedure, TableInput, compile};
pub(crate) use statements::run_statement;

use execute::{Execution, execute};

use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::exec::Subqueries;
use crate::storage::Row;
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §9.24, §9.25] An invocation of a polymorphic table
/// function, compiled, as a plan runs it: its PTF data area as
/// compilation left it, its table arguments, and its start, fulfill and
/// finish component procedures, bound.
#[derive(Debug, Clone)]
pub(crate) struct Call {
    /// The function's name.
    pub(crate) name: QualifiedName,
    /// The invocation, compiled.
    pub(crate) compiled: Compiled,
    /// Its table arguments, bound.
    pub(crate) tables: Vec<TableInput>,
    /// Its PTF start component procedure, if it has one.
    pub(crate) start: Option<Procedure>,
    /// Its PTF fulfill component procedure.
    pub(crate) fulfill: Procedure,
    /// Its PTF finish component procedure, if it has one.
    pub(crate) finish: Option<Procedure>,
}

impl Call {
    /// The programs of its start, fulfill and finish component procedures,
    /// which run as its plan runs.
    pub(crate) fn programs(&self) -> impl Iterator<Item = &crate::logical::Program> {
        [self.start.as_ref(), Some(&self.fulfill), self.finish.as_ref()].into_iter().flatten().map(|p| &*p.program)
    }

    /// [ISO/IEC 9075-2:2023 §9.25] Its result, of the values `arguments` of
    /// its pre-compilation argument list and its table arguments' rows
    /// `rows`, where `subqueries` runs a query's subqueries.
    ///
    /// # Errors
    ///
    /// As `execute`'s.
    pub(crate) fn run(&self, arguments: Vec<Value>, rows: &[Vec<Row>], subqueries: &dyn Subqueries) -> Result<Vec<Row>> {
        let x = Execution {
            compiled: &self.compiled,
            tables: &self.tables,
            start: self.start.as_ref(),
            fulfill: &self.fulfill,
            finish: self.finish.as_ref(),
            arguments,
        };
        execute(&x, rows, subqueries)
    }
}
