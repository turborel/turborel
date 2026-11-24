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
mod graph;
mod statements;

pub(crate) use area::DataArea;
pub(crate) use compile::{Compiled, DescriptorColumns, Procedure, TableInput, compile};
pub(crate) use graph::{EdgeShape, GraphCall, GraphShape, identity_type};
pub(crate) use statements::run_statement;

use execute::{Execution, execute};

use std::sync::Arc;

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::Subqueries;
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Value;

/// What a `<PTF derived table>` invokes: a polymorphic table function
/// [ISO/IEC 9075-2:2023 §9.25], or a function of GRAPH_ALGO
/// (\[EXTENSION\], `graph.rs`).
#[derive(Debug, Clone)]
pub(crate) enum Invoked {
    /// A polymorphic table function's invocation, compiled.
    Routine(Box<Call>),
    /// A graph algorithm's.
    Graph(GraphCall),
}

impl Invoked {
    /// The function's name.
    pub(crate) fn name(&self) -> &QualifiedName {
        match self {
            Invoked::Routine(c) => &c.name,
            Invoked::Graph(g) => &g.name,
        }
    }

    /// The programs of its component procedures, which run as its plan
    /// runs: none of a graph algorithm.
    pub(crate) fn programs(&self) -> impl Iterator<Item = &crate::logical::Program> {
        let routine = match self {
            Invoked::Routine(c) => Some(&**c),
            Invoked::Graph(_) => None,
        };
        routine.into_iter().flat_map(Call::programs)
    }

    /// Its result, of the values `arguments` of its arguments and its
    /// table arguments' rows `rows`, where `subqueries` runs a query's
    /// subqueries, run in the database and for the statement of the
    /// transaction `run_in` gives.
    ///
    /// # Errors
    ///
    /// As `Call::run`'s or `GraphCall::run`'s.
    pub(crate) fn run(
        &self,
        arguments: Vec<Value>,
        rows: &[Vec<Row>],
        subqueries: &dyn Subqueries,
        run_in: (&Arc<State>, TxId),
    ) -> Result<Vec<Row>> {
        match self {
            Invoked::Routine(c) => c.run(arguments, rows, subqueries),
            Invoked::Graph(g) => g.run(arguments, rows, run_in),
        }
    }
}

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
