//! \[DESIGN\] The settings of parallel queries (RULES.md D290, D291): the
//! executor's workers, shared by every statement of a database, and what a
//! statement asks of them, its degree, the rows of its morsels and the
//! least rows it reads in parallel; and whether a hash join filters its
//! probing rows by its built ones' keys (RULES.md D369).

use super::builtin::{ON, SWITCHES, at_least, count, defined, text};
use super::{Definition, Effect, Key, Kind};

/// EXECUTOR.WORKERS.
pub const EXECUTOR_WORKERS: Key = Key(82);
/// EXECUTOR.DEGREE.
pub const EXECUTOR_DEGREE: Key = Key(83);
/// EXECUTOR.MORSEL_ROWS.
pub const EXECUTOR_MORSEL_ROWS: Key = Key(84);
/// EXECUTOR.PARALLEL_MIN_ROWS.
pub const EXECUTOR_PARALLEL_MIN_ROWS: Key = Key(85);
/// EXECUTOR.RUNTIME_FILTERS.
pub const EXECUTOR_RUNTIME_FILTERS: Key = Key(86);

/// \[DESIGN\] The default of EXECUTOR.MORSEL_ROWS: enough rows that taking a
/// morsel costs little beside reading it, few enough that the last
/// morsels of a scan even out among its workers (Leis et al.,
/// "Morsel-Driven Parallelism", 2014, §3.3, measure the trade).
pub const DEFAULT_MORSEL_ROWS: i128 = 16_384;

/// \[DESIGN\] The default of EXECUTOR.PARALLEL_MIN_ROWS: four morsels, below
/// which a statement's own thread reads its table as soon as workers
/// would, a point query's never offered to them.
pub const DEFAULT_PARALLEL_MIN_ROWS: i128 = 65_536;

/// A database's setting, from the statements that begin after it changes.
const DATABASE: (bool, Effect) = (false, Effect::Statement);

/// A setting a session may set for itself, from its next statement.
const SESSION: (bool, Effect) = (true, Effect::Statement);

/// The settings of parallel queries, in the order of their keys.
pub(super) fn definitions() -> [Definition; 5] {
    [
        defined(
            ("EXECUTOR.WORKERS", at_least(0), count(0)),
            DATABASE,
            "The executor's workers, threads shared by every statement of the database that runs in parallel; 0, the default, for as many as the system says it runs at once.",
        ),
        defined(
            ("EXECUTOR.DEGREE", at_least(0), count(0)),
            SESSION,
            "The most threads a statement's query runs on at once, its own among them; 0, the default, for as many as the workers, and 1 for its own thread alone. Its rows are the same either way.",
        ),
        defined(
            ("EXECUTOR.MORSEL_ROWS", at_least(1), count(DEFAULT_MORSEL_ROWS)),
            SESSION,
            "The rows of a table, about, that a worker of a parallel query reads at a time, a morsel.",
        ),
        defined(
            ("EXECUTOR.PARALLEL_MIN_ROWS", at_least(0), count(DEFAULT_PARALLEL_MIN_ROWS)),
            SESSION,
            "The least rows a table a query reads holds, as the planner estimates, for the query to read it on workers; a smaller one is read by the statement's own thread alone. 0 for any table.",
        ),
        defined(
            ("EXECUTOR.RUNTIME_FILTERS", Kind::Choice(SWITCHES), text(ON)),
            SESSION,
            "Whether a hash join, once it has built its right operand, tells the scan of its left operand the least and greatest of its keys and filters the rows read by a Bloom filter of them: ON; or not, OFF, as to compare the two. Its rows are the same either way.",
        ),
    ]
}
