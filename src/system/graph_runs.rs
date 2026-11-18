//! \[EXTENSION\] INFORMATION_SCHEMA.GRAPH_ALGO_RUNS, of the graph
//! standard's §20 (RULES.md D-GRAPH-ALGO): the latest runs of the
//! functions of GRAPH_ALGO, each its number, algorithm, arguments, user,
//! start and time taken, its graph's vertices and edges, its rows, its
//! threads and how it ended. The log is the database's, in memory, of the
//! latest `KEPT` runs: the database's owner sees them all, any other user
//! their own.
//!
//! It holds too the result of a run a statement reads more than once: an
//! INTO PROPERTY of a property graph, whose MERGE of each vertex table
//! reads one invocation. While the statement holds its transaction's
//! place (`GraphRuns::hold`), the first run's rows are kept and the others
//! read them, so the algorithm runs once and is noted once.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::{Seen, SystemTable, identifier, text, text_column};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::datetime::Moment;
use crate::value::datetime::utc_of_unix;
use crate::value::{DataType, Datetime, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

/// \[DESIGN\] The runs the log keeps, the oldest dropped past them.
pub const KEPT: usize = 1_000;
/// \[DESIGN\] The longest text of a run's arguments or message shown.
const TEXT_LENGTH: u32 = 1024;
/// The longest STATUS, FAILED.
const STATUS_LENGTH: u32 = 6;
/// The digits of the seconds' fraction of STARTED.
const PRECISION: u8 = 6;

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// It gave its result.
    Done,
    /// It failed with the message.
    Failed(String),
}

/// \[EXTENSION\] A run of a function of GRAPH_ALGO, as the log notes it.
#[derive(Debug, Clone)]
pub struct GraphRun {
    /// Its number, given as the log notes it, from 1.
    pub id: u64,
    /// Its algorithm's name.
    pub algorithm: &'static str,
    /// Its value arguments, each `NAME => value`.
    pub arguments: String,
    /// Who ran it.
    pub user: String,
    /// When it started, in microseconds since 1970-01-01 UTC.
    pub started: i64,
    /// How long it took.
    pub elapsed: Duration,
    /// Its graph's vertices.
    pub vertices: usize,
    /// Its graph's edges.
    pub edges: usize,
    /// Its result's rows.
    pub rows: usize,
    /// The threads it ran on.
    pub threads: usize,
    /// How it ended.
    pub status: Status,
}

impl GraphRun {
    /// A run of `algorithm` with `arguments` by `user` started at
    /// `started`, done, of nothing yet.
    pub fn new(algorithm: &'static str, arguments: String, user: &str, started: i64) -> GraphRun {
        GraphRun {
            id: 0,
            algorithm,
            arguments,
            user: user.to_owned(),
            started,
            elapsed: Duration::ZERO,
            vertices: 0,
            edges: 0,
            rows: 0,
            threads: 1,
            status: Status::Done,
        }
    }
}

/// \[EXTENSION\] The database's log of its latest runs.
#[derive(Debug, Default)]
pub struct GraphRuns {
    log: Mutex<(u64, VecDeque<GraphRun>)>,
    /// Of each transaction whose statement holds results, the invocation
    /// held and its rows, once run.
    held: Mutex<HashMap<TxId, Option<HeldRows>>>,
}

/// An invocation held, as its text, and the rows it gave.
type HeldRows = (String, Arc<Vec<Row>>);

/// \[EXTENSION\] A statement's hold of the results of transaction `tx`'s
/// runs: given up as it drops.
pub struct Held {
    runs: Arc<GraphRuns>,
    tx: TxId,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.runs.held.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.tx);
    }
}

impl GraphRuns {
    /// Notes `run`, numbered after the last, the oldest dropped past
    /// `KEPT`.
    pub fn note(&self, mut run: GraphRun) {
        let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        log.0 = log.0.saturating_add(1);
        run.id = log.0;
        if log.1.len() >= KEPT {
            log.1.pop_front();
        }
        log.1.push_back(run);
    }

    /// A hold of the results of `tx`'s runs until it drops: the first
    /// run's rows kept, each run after of the same invocation given them.
    pub fn hold(self: &Arc<Self>, tx: TxId) -> Held {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).insert(tx, None);
        Held { runs: Arc::clone(self), tx }
    }

    /// The rows held of `tx`'s invocation `call`, if they are.
    pub fn held(&self, tx: TxId, call: &str) -> Option<Arc<Vec<Row>>> {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.get(&tx).and_then(Option::as_ref).filter(|(c, _)| c == call).map(|(_, rows)| Arc::clone(rows))
    }

    /// Keeps `rows`, of `tx`'s invocation `call`, if a hold of `tx`'s has
    /// none yet.
    pub fn keep(&self, tx: TxId, call: &str, rows: &[Row]) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(slot @ None) = held.get_mut(&tx) {
            *slot = Some((call.to_owned(), Arc::new(rows.to_vec())));
        }
    }

    /// The runs kept, oldest first.
    pub fn runs(&self) -> Vec<GraphRun> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).1.iter().cloned().collect()
    }
}

/// \[EXTENSION\] INFORMATION_SCHEMA.GRAPH_ALGO_RUNS.
pub(super) struct GraphAlgoRuns;

/// A column of a count, not null.
fn count_column(name: &str) -> ColumnDesc {
    ColumnDesc {
        name: name.to_owned(),
        data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
        not_null: true,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    }
}

/// A count's value.
fn count(n: impl Into<i128>) -> Value {
    Value::Exact(Decimal::new(n.into(), 0))
}

/// The text `s`, cut at `TEXT_LENGTH` characters.
fn cut(s: &str) -> Value {
    text(&s.chars().take(usize::try_from(TEXT_LENGTH).unwrap_or(usize::MAX)).collect::<String>())
}

/// A size's value.
fn size(n: usize) -> Value {
    count(i128::try_from(n).unwrap_or(i128::MAX))
}

/// The row of `run`.
fn row(run: &GraphRun) -> Row {
    let started = Datetime { moment: Moment::Timestamp, utc: utc_of_unix(run.started), zone: 0, zoned: true, precision: PRECISION };
    let (status, message) = match &run.status {
        Status::Done => ("DONE", Value::Null),
        Status::Failed(m) => ("FAILED", cut(m)),
    };
    vec![
        count(run.id),
        text(run.algorithm),
        cut(&run.arguments),
        text(&run.user),
        Value::Datetime(started),
        count(run.elapsed.as_micros().try_into().unwrap_or(i128::MAX)),
        size(run.vertices),
        size(run.edges),
        size(run.rows),
        size(run.threads),
        text(status),
        message,
    ]
}

impl SystemTable for GraphAlgoRuns {
    fn name(&self) -> &'static str {
        "GRAPH_ALGO_RUNS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let started =
            ColumnDesc { data_type: DataType::Timestamp { precision: u32::from(PRECISION), zoned: true }, ..count_column("STARTED") };
        vec![
            count_column("RUN_ID"),
            identifier("ALGORITHM", true),
            text_column(("ARGUMENTS", TEXT_LENGTH), true),
            identifier("USER_NAME", true),
            started,
            count_column("ELAPSED_MICROS"),
            count_column("VERTICES"),
            count_column("EDGES"),
            count_column("ROWS_RETURNED"),
            count_column("THREADS"),
            text_column(("STATUS", STATUS_LENGTH), true),
            text_column(("MESSAGE", TEXT_LENGTH), false),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let owner = seen.is_owner();
        Ok(seen.state.graph_runs.runs().iter().filter(|r| owner || r.user == seen.auth).map(row).collect())
    }
}
