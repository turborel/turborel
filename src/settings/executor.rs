//! \[DESIGN\] The settings of parallel queries (RULES.md D290, D291): the
//! executor's workers, shared by every statement of a database, and what a
//! statement asks of them, its degree, the rows of its morsels and the
//! least rows it reads in parallel.

use super::builtin::{at_least, count, defined, text};
use super::{Definition, Effect, Key, Kind};

/// EXECUTOR.WORKERS.
pub const EXECUTOR_WORKERS: Key = Key(82);
/// EXECUTOR.DEGREE.
pub const EXECUTOR_DEGREE: Key = Key(83);
/// EXECUTOR.MORSEL_ROWS.
pub const EXECUTOR_MORSEL_ROWS: Key = Key(84);
/// EXECUTOR.PARALLEL_MIN_ROWS.
pub const EXECUTOR_PARALLEL_MIN_ROWS: Key = Key(85);
/// EXECUTOR.KERNELS (RULES.md D460).
pub const EXECUTOR_KERNELS: Key = Key(86);

/// The most bytes of EXECUTOR.KERNELS: a kernels set's name.
const KERNELS_NAME_MAX: usize = 64;

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
            ("EXECUTOR.KERNELS", Kind::Text { max: KERNELS_NAME_MAX }, text("")),
            SESSION,
            "The kernels a statement's batches run on in the executor (filters, groupings, sorts): PORTABLE for the built-in ones, as to diagnose others; empty, the default, or the registered set's name, for the set registered, such as SIMD. A column store table decodes and judges its scans on the set registered, which it takes as it opens; the setting does not change it.",
        ),
    ]
}
