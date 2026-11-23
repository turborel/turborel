//! \[DESIGN\] The settings of parallel queries (RULES.md D290, D291): the
//! executor's workers, shared by every statement of a database, and what a
//! statement asks of them, its degree, the rows of its morsels and the
//! least rows it reads in parallel; and whether a hash join filters its
//! probing rows by its built ones' keys (RULES.md D369); and whether a
//! correlated subquery keeps its plan across its runs (RULES.md D675);
//! and whether a hash join of dense keys finds its rows by their places
//! (RULES.md D601); and whether a kept point statement runs straight on
//! its table (RULES.md D752).

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
/// EXECUTOR.KERNELS (RULES.md D460).
pub const EXECUTOR_KERNELS: Key = Key(86);
/// EXECUTOR.RUNTIME_FILTERS.
pub const EXECUTOR_RUNTIME_FILTERS: Key = Key(87);
/// EXECUTOR.KEEP_SUBQUERY_PLANS (RULES.md D675).
pub const KEEP_SUBQUERY_PLANS: Key = Key(89);
/// EXECUTOR.DENSE_JOINS (RULES.md D601), at its key's place after the
/// planner's estimate margin.
pub const EXECUTOR_DENSE_JOINS: Key = Key(92);
/// EXECUTOR.POINT_OPERATIONS (RULES.md D752), at its key's place after
/// the escrow of TRANSACTION.ESCROW.
pub const EXECUTOR_POINT_OPERATIONS: Key = Key(94);
/// EXECUTOR.COMPILED_PROCEDURES (RULES.md D839), at its key's place after
/// the planner's predicate transfer.
pub const COMPILED_PROCEDURES: Key = Key(96);

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
pub(super) fn definitions() -> [Definition; 6] {
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
        defined(
            ("EXECUTOR.RUNTIME_FILTERS", Kind::Choice(SWITCHES), text(ON)),
            SESSION,
            "Whether a hash join, once it has built its right operand, tells the scan of its left operand the least and greatest of its keys and filters the rows read by a Bloom filter of them: ON; or not, OFF, as to compare the two. Its rows are the same either way.",
        ),
    ]
}

/// EXECUTOR.KEEP_SUBQUERY_PLANS, at its key's place after the planner's
/// range joins.
pub(super) fn kept_subquery_plans() -> Definition {
    defined(
        ("EXECUTOR.KEEP_SUBQUERY_PLANS", Kind::Choice(SWITCHES), text(ON)),
        SESSION,
        "Whether a correlated subquery, once its first runs show the values of its outer references choose no other plan, keeps its plan made with them as parameters and fills it at each later run: ON; or plans it at each run with its values, OFF, as to compare the two. Its rows are the same either way.",
    )
}

/// EXECUTOR.DENSE_JOINS (RULES.md D601).
pub(super) fn dense_joins() -> Definition {
    defined(
        ("EXECUTOR.DENSE_JOINS", Kind::Choice(SWITCHES), text(ON)),
        SESSION,
        "Whether a hash join whose one key is of whole numbers dense in their span, at most four times as wide as its built rows, finds a probe's rows at its key's place in the span, no hash made: ON; or hashes every key, OFF, as to compare the two. Its rows are the same either way.",
    )
}

/// EXECUTOR.POINT_OPERATIONS (RULES.md D752).
pub(super) fn point_operations() -> Definition {
    defined(
        ("EXECUTOR.POINT_OPERATIONS", Kind::Choice(SWITCHES), text(ON)),
        SESSION,
        "Whether a kept query, UPDATE or DELETE whose plan is one probe of a whole unique key reads its row straight from the table: ON; or through the executor's operators, OFF, as to compare the two. Its rows, errors and diagnostics are the same either way.",
    )
}

/// EXECUTOR.COMPILED_PROCEDURES (RULES.md D839).
pub(super) fn compiled_procedures() -> Definition {
    defined(
        ("EXECUTOR.COMPILED_PROCEDURES", Kind::Choice(SWITCHES), text(ON)),
        SESSION,
        "Whether the body of an SQL-invoked procedure a CALL invokes runs as the program it is compiled to as it is first called, its point reads and point changes straight on their tables: ON; or statement by statement through the general path, OFF, as to compare the two. Its results, errors, warnings and diagnostics are the same either way.",
    )
}
