//! \[EXTENSION\] An invocation of a function of GRAPH_ALGO as a plan runs
//! it (RULES.md D111): the library's algorithm run on its table
//! arguments' rows (`graph/algo/library.rs`), on as many threads as
//! EXECUTOR.GRAPH_THREADS says, its arrays held within MEMORY.WORK_BYTES,
//! stopped as the statement is cancelled; each run noted in
//! INFORMATION_SCHEMA.GRAPH_ALGO_RUNS (`system/graph_runs.rs`).
//!
//! The rows are the statement's, read in its snapshot, so the graph is a
//! consistent one [GS §18 R18.1], and through the privileges and the
//! views its table arguments are read with [R18.2].

mod shape;

pub(crate) use crate::logical::identity_type;
pub(crate) use shape::{EdgeShape, GraphShape};

use std::sync::Arc;
use std::time::Instant;

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Result, SqlError};
use crate::graph::algo::{Algorithm, Arguments, Control, Input, Memory, Sizes, run};
use crate::settings::{GRAPH_THREADS, MEMORY_WORK_BYTES, work_bytes};
use crate::storage::Row;
use crate::system::{GraphRun, Status};
use crate::txn::TxId;
use crate::value::Value;

/// \[EXTENSION\] An invocation of a function of GRAPH_ALGO, bound.
#[derive(Debug, Clone)]
pub(crate) struct GraphCall {
    /// The function's name.
    pub(crate) name: QualifiedName,
    /// Its algorithm.
    pub(crate) algorithm: Algorithm,
    /// Of each value argument, in the order of the plan's arguments, the
    /// name of its parameter.
    pub(crate) parameters: Vec<&'static str>,
    /// Whether a table argument of the vertices follows the edges'.
    pub(crate) vertices: bool,
    /// The names of the edges' columns, of which COST names one.
    pub(crate) edge_columns: Vec<String>,
    /// Of a property graph named in place of the edges, how its table
    /// arguments' rows are made its edges and vertices.
    pub(crate) shape: Option<GraphShape>,
    /// Who invoked it.
    pub(crate) user: String,
}

/// GRF-0034 of a COST that names no column.
fn no_column(cost: &str) -> SqlError {
    crate::graph::algo::cost_refused(&format!("COST names {cost}, which is no column of the edges"))
}

impl GraphCall {
    /// The place among the edges' columns of the column `cost` names: as
    /// written, else of its upper case.
    fn cost_column(&self, cost: &str) -> Result<usize> {
        let at = |name: &str| self.edge_columns.iter().position(|c| c == name);
        at(cost).or_else(|| at(&cost.to_uppercase())).ok_or_else(|| no_column(cost))
    }

    /// Its arguments' values, `values` in the order of its parameters,
    /// shown as GRAPH_ALGO_RUNS shows them, those not given left out.
    fn shown(&self, values: &[Value]) -> String {
        let shown: Vec<String> =
            self.parameters.iter().zip(values).filter(|(_, v)| !v.is_null()).map(|(p, v)| format!("{p} => {}", v.to_literal())).collect();
        shown.join(", ")
    }

    /// Its result, of its value arguments' values `values` and its table
    /// arguments' rows `inputs`, run in `db` for `tx`'s statement, the run
    /// noted.
    ///
    /// # Errors
    ///
    /// The run's (`library::run`); SQLCODE -301 (GRF-0034) if COST names
    /// no column of the edges.
    pub(crate) fn run(&self, values: Vec<Value>, inputs: &[Vec<Row>], (db, tx): (&Arc<State>, TxId)) -> Result<Vec<Row>> {
        // An invocation its statement reads again, of the rows it gave.
        let call = format!("{}({}) {:?}", self.algorithm.name(), self.shown(&values), self.edge_columns);
        if let Some(rows) = db.graph_runs.held(tx, &call) {
            return Ok(rows.to_vec());
        }
        let started = Instant::now();
        let mut noted = GraphRun::new(self.algorithm.name(), self.shown(&values), &self.user, db.clock.now());
        let threads = db.settings.count(GRAPH_THREADS).max(1);
        noted.threads = threads;
        let found = self.found(values, inputs, (db, tx, threads));
        noted.elapsed = started.elapsed();
        let rows = match found {
            Ok((rows, sizes)) => {
                (noted.vertices, noted.edges, noted.rows) = (sizes.vertices, sizes.edges, rows.len());
                db.graph_runs.keep(tx, &call, &rows);
                Ok(rows)
            }
            Err(e) => {
                noted.status = Status::Failed(e.msg.clone());
                Err(e)
            }
        };
        db.graph_runs.note(noted);
        rows
    }

    /// `run`'s result and the graph's sizes, on `threads` threads.
    fn found(&self, values: Vec<Value>, rows: &[Vec<Row>], (db, tx, threads): (&Arc<State>, TxId, usize)) -> Result<(Vec<Row>, Sizes)> {
        let arguments = Arguments(self.parameters.iter().copied().zip(values).collect());
        let cost =
            arguments.0.iter().find_map(|(p, v)| if let (true, Value::Char(c)) = (*p == "COST", v) { Some(c.clone()) } else { None });
        let of_graph = self.shape.as_ref().map(|shape| shape.rows(rows)).transpose()?;
        let (edges, vertices) = match &of_graph {
            Some((edges, vertices)) => (edges.as_slice(), Some(vertices.as_slice())),
            None => (rows.first().map_or(&[][..], Vec::as_slice), if self.vertices { rows.get(1).map(Vec::as_slice) } else { None }),
        };
        let input = Input { edges, vertices, cost: cost.as_deref().map(|c| self.cost_column(c)).transpose()? };
        let memory = Memory::new(work_bytes(db.settings.get(MEMORY_WORK_BYTES)));
        let stop = db.tx.stop_of(tx);
        let cancel = || stop.as_ref().map_or(Ok(()), |s| s.check());
        run(self.algorithm, &arguments, &input, &Control::new(threads, &memory, &cancel))
    }
}
