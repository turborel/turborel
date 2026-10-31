//! \[DESIGN\] The built-in settings (RULES.md D36), each at its key's place,
//! in groups: transactions and definitions, planning, the catalog's name,
//! memory, the planner's guesses, spilling, durability, storage, limits,
//! the executor, the threads of background work (RULES.md D55), and the
//! log's own pace of syncs. A default its subsystem names by a constant is
//! that constant.
//! Split from settings.rs (RULES.md R-SIZE-2).

use super::{Definition, Effect, Key, Kind};
use crate::value::{Decimal, Value};

/// The built-in settings, each at its key's place (`builtin`).
pub const TABLES_ENGINE: Key = Key(0);
/// See [`TABLES_ENGINE`].
pub const TABLES_UNIQUE_INDEX_METHOD: Key = Key(1);
/// See [`TABLES_ENGINE`].
pub const TRANSACTION_STRICT: Key = Key(2);
/// See [`TABLES_ENGINE`].
pub const LOCKS_WAIT: Key = Key(3);
/// See [`TABLES_ENGINE`].
pub const SESSION_COMMIT_AT_END: Key = Key(4);
/// See [`TABLES_ENGINE`].
pub const OPTIMIZER_MAX_PASSES: Key = Key(5);
/// See [`TABLES_ENGINE`].
pub const INDEX_DESCENT_COST: Key = Key(6);
/// See [`TABLES_ENGINE`].
pub const SORT_COMPARE_COST: Key = Key(7);
/// See [`TABLES_ENGINE`].
pub const CATALOG_NAME: Key = Key(8);
/// See [`TABLES_ENGINE`].
pub const MEMORY_BUDGET_BYTES: Key = Key(9);
/// See [`TABLES_ENGINE`].
pub const MEMORY_WORK_BYTES: Key = Key(10);
/// See [`TABLES_ENGINE`].
pub const SPILL_BATCH_BYTES: Key = Key(11);
/// See [`TABLES_ENGINE`].
pub const EQUAL_SELECTIVITY: Key = Key(12);
/// See [`TABLES_ENGINE`].
pub const RANGE_SELECTIVITY: Key = Key(13);
/// See [`TABLES_ENGINE`].
pub const NULL_FRACTION: Key = Key(14);
/// See [`TABLES_ENGINE`].
pub const REORDER_GAIN: Key = Key(15);
/// See [`TABLES_ENGINE`].
pub const DECORRELATE_AFTER_RUNS: Key = Key(16);
/// See [`TABLES_ENGINE`].
pub const SPILL_PARTITIONS: Key = Key(17);
/// See [`TABLES_ENGINE`].
pub const SPILL_PARTITION_DEPTH: Key = Key(18);
/// See [`TABLES_ENGINE`].
pub const CHECKPOINT_AFTER_CHANGES: Key = Key(19);
/// See [`TABLES_ENGINE`].
pub const CHECKPOINT_AFTER_BYTES: Key = Key(20);
/// See [`TABLES_ENGINE`].
pub const CATALOG_COMPACT_AFTER_IMAGES: Key = Key(21);
/// See [`TABLES_ENGINE`].
pub const PAGE_BYTES: Key = Key(22);
/// See [`TABLES_ENGINE`].
pub const CHECKSUM: Key = Key(23);
/// See [`TABLES_ENGINE`].
pub const ENCODINGS: Key = Key(24);
/// See [`TABLES_ENGINE`].
pub const LOG_SEGMENT_BYTES: Key = Key(25);
/// See [`TABLES_ENGINE`].
pub const POOL_BYTES: Key = Key(26);
/// See [`TABLES_ENGINE`].
pub const STATEMENT_TIMEOUT_MS: Key = Key(27);
/// See [`TABLES_ENGINE`].
pub const LOCKS_TIMEOUT_MS: Key = Key(28);
/// See [`TABLES_ENGINE`].
pub const SLOW_STATEMENT_MS: Key = Key(29);
/// See [`TABLES_ENGINE`].
pub const SESSION_TIME_ZONE: Key = Key(30);
/// See [`TABLES_ENGINE`].
pub const EXECUTOR_BATCHES: Key = Key(31);
/// See [`TABLES_ENGINE`].
pub const JOBS_POLICY: Key = Key(32);
/// See [`TABLES_ENGINE`].
pub const FILES_MAX_OPEN: Key = Key(33);
/// See [`TABLES_ENGINE`].
pub const DIAGNOSTICS_LEVEL: Key = Key(34);
/// See [`TABLES_ENGINE`].
pub const SPILL_ENCODINGS: Key = Key(35);
/// See [`TABLES_ENGINE`].
pub const COMMIT_SYNC: Key = Key(36);
/// See [`TABLES_ENGINE`].
pub const FUNCTION_CALLS: Key = Key(37);
/// See [`TABLES_ENGINE`].
pub const CHECKPOINT_OVERDUE_FACTOR: Key = Key(38);
/// See [`TABLES_ENGINE`].
pub const LOG_SYNC_AFTER_MS: Key = Key(39);

/// The words of a two-way choice (RULES.md D36): of `EXECUTOR.BATCHES`,
/// `LOCKS.WAIT`, `LOG.COMMIT_SYNC`, `SQL.FUNCTION_CALLS`, each optimizer
/// rule's, and the others.
pub const SWITCHES: &[&str] = &[ON, OFF];
/// The word of a two-way choice taken.
pub const ON: &str = "ON";
/// The word of a two-way choice not taken.
pub const OFF: &str = "OFF";

/// A character string value.
pub(super) fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

/// A whole number value.
pub(super) fn whole(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// `n` as a whole number of a setting.
fn count(n: impl TryInto<i128>) -> Value {
    whole(n.try_into().unwrap_or(i128::MAX))
}

/// A whole number from `min` up.
fn at_least(min: i128) -> Kind {
    Kind::Integer { min, max: i128::from(i64::MAX) }
}

/// A fraction, from 0 to 1.
const FRACTION: Kind = Kind::Number { min: 0.0, max: 1.0 };

/// One built-in setting.
fn defined((name, kind, default): (&str, Kind, Value), (per_session, effect): (bool, Effect), doc: &str) -> Definition {
    Definition { name: name.to_owned(), kind, default, per_session, effect, doc: doc.to_owned() }
}

/// A database's setting, taking effect for the statements that begin after
/// it changes.
const DATABASE: (bool, Effect) = (false, Effect::Statement);

/// A setting a session may set for itself, over the database's, taking
/// effect for the statements that begin after it changes.
const SESSION: (bool, Effect) = (true, Effect::Statement);

/// The built-in settings, in the order of their keys.
pub(super) fn builtin() -> Vec<Definition> {
    let mut all = Vec::from(definitions());
    all.extend(cost());
    all.push(defined(
        ("CATALOG.NAME", Kind::Identifier, text("TURBOREL")),
        DATABASE,
        "The name of the catalog the schemata are in, as the Information Schema gives it [ISO/IEC 9075:1992 §21.2.2].",
    ));
    all.extend(memory());
    all.extend(guesses());
    all.extend(spilling());
    all.extend(durability());
    all.extend(storage());
    all.extend(limits());
    all.push(defined(
        ("EXECUTOR.BATCHES", Kind::Choice(SWITCHES), text(ON)),
        SESSION,
        "Whether a query reads its tables a batch of rows at a time where it can: ON; or a row at a time throughout, OFF, as to compare the two.",
    ));
    all.push(defined(
        ("JOBS.POLICY", Kind::Choice(&crate::threads::JobPolicy::WORDS), text("AUTOMATIC")),
        DATABASE,
        "How the database's background work, its checkpoints, maintenance and syncs of the log (LOG.SYNC_AFTER_MS), runs: on threads of its own as it comes due, AUTOMATIC; only when the program asks, MANUAL; or not until this changes, PAUSED.",
    ));
    all.push(defined(
        ("FILES.MAX_OPEN", Kind::Integer { min: 0, max: i128::from(u32::MAX) }, whole(i128::try_from(crate::files::DEFAULT_MAX_OPEN).unwrap_or(1024))),
        DATABASE,
        "The most files of the database open at once, 0 for no limit: past it, the one used least recently is closed (one with writes not yet synced last, and synced first), and opened again as it is next used.",
    ));
    all.push(defined(
        ("DIAGNOSTICS.LEVEL", Kind::Choice(&crate::diagnostics::Level::WORDS), text("INFO")),
        DATABASE,
        "The least level of the events told to the diagnostics: ERROR, those alone; WARNING, warnings too; INFO, every event.",
    ));
    all.push(defined(
        ("SPILL.ENCODINGS", Kind::Encodings, text("")),
        DATABASE,
        "The encodings, as compression, the files of work areas made from now on pass through, by commas, in order, before the encryption they pass through while the database encrypts anything; none if empty.",
    ));
    all.extend(switches());
    all.push(defined(
        ("CHECKPOINT.OVERDUE_FACTOR", Kind::Integer { min: 1, max: 1_000_000 }, count(crate::db::OVERDUE)),
        DATABASE,
        "How many times CHECKPOINT.AFTER_CHANGES or CHECKPOINT.AFTER_BYTES the log, or the data file's side file, grows to with no checkpoint giving it up before the diagnostics are told that checkpoints are overdue.",
    ));
    all.push(defined(
        ("LOG.SYNC_AFTER_MS", at_least(0), count(crate::db::SYNC_AFTER_MS)),
        DATABASE,
        "The milliseconds after a commit of LOG.COMMIT_SYNC OFF within which the log is synced, by a job of its own if nothing else syncs it first; 0 for none, the commit then synced only by the log's next sync for another reason.",
    ));
    all
}

/// \[DESIGN\] What a session may turn on or off for itself: whether its
/// commits wait for the log, and whether it takes the function call
/// syntax.
fn switches() -> [Definition; 2] {
    [
        defined(
            ("LOG.COMMIT_SYNC", Kind::Choice(SWITCHES), text(ON)),
            SESSION,
            "Whether a commit waits until its commit record is synced to the log, ON; or returns once the record is written, OFF: the transaction is then seen by others at once, and lost if the system fails before the log is next synced, by a commit of ON, a definition, a checkpoint, the database closing, or LOG.SYNC_AFTER_MS passing.",
        ),
        defined(
            ("SQL.FUNCTION_CALLS", Kind::Choice(SWITCHES), text(OFF)),
            SESSION,
            "Whether a statement may call a function by the syntax NAME(arguments), which the standards do not have: ON; or refuses it, as SQL does, OFF.",
        ),
    ]
}

/// \[DESIGN\] A database's files: their formats, fixed when it is created,
/// and its buffer pool, made as it is opened.
fn storage() -> [Definition; 5] {
    use crate::disk::pager::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, MIN_PAGE_SIZE};
    use crate::log::{DEFAULT_SEGMENT_SIZE, MAX_SEGMENT_SIZE, MIN_SEGMENT_SIZE};
    let created = (false, Effect::Creation);
    [
        defined(
            ("STORAGE.PAGE_BYTES", Kind::Integer { min: MIN_PAGE_SIZE.into(), max: MAX_PAGE_SIZE.into() }, count(DEFAULT_PAGE_SIZE)),
            created,
            "The bytes of a page of the data file, a power of two.",
        ),
        defined(
            ("STORAGE.CHECKSUM", Kind::HashFunction, text(crate::hash::crc::CRC32_ISCSI.name)),
            created,
            "The hash function every file is checksummed with.",
        ),
        defined(
            ("STORAGE.ENCODINGS", Kind::Encodings, text("")),
            created,
            "The encodings, as compression or encryption, every file passes through, by commas, in order; none if empty.",
        ),
        defined(
            (
                "LOG.SEGMENT_BYTES",
                Kind::Integer { min: MIN_SEGMENT_SIZE.into(), max: MAX_SEGMENT_SIZE.into() },
                count(DEFAULT_SEGMENT_SIZE),
            ),
            created,
            "The bytes after which a segment of the log is ended and the next begun.",
        ),
        defined(
            ("STORAGE.POOL_BYTES", at_least(1_i128 << 16_i32), count(crate::db::POOL_BYTES)),
            (false, Effect::Start),
            "The bytes of the buffer pool of the data file, reserved of MEMORY.BUDGET_BYTES as the database is opened; of a database kept in memory, the pool its engine and index methods share, made with its first table.",
        ),
    ]
}

/// \[DESIGN\] How long a statement, or a wait for a lock, may last, and how
/// long one may run before the diagnostics are told of it: in
/// milliseconds, 0 for no limit.
fn limits() -> [Definition; 4] {
    [
        defined(
            ("STATEMENT.TIMEOUT_MS", at_least(0), whole(0)),
            SESSION,
            "The milliseconds a statement may run, each call and each fetch of a cursor alone, before it is cancelled (SQLCODE -920); 0 for no limit.",
        ),
        defined(
            ("LOCKS.TIMEOUT_MS", at_least(0), whole(0)),
            SESSION,
            "The milliseconds a statement waits for a lock, under LOCKS.WAIT ON, before it is refused (SQLCODE -911); 0 for no limit.",
        ),
        defined(
            ("DIAGNOSTICS.SLOW_STATEMENT_MS", at_least(0), whole(0)),
            DATABASE,
            "The milliseconds of work after which a statement is told to the diagnostics as slow; 0 for none.",
        ),
        defined(
            ("SESSION.TIME_ZONE", Kind::Displacement, text("+00:00")),
            SESSION,
            "The time zone displacement of an SQL-session, ±HH:MM from -12:59 to +13:00 [ISO/IEC 9075:1992 §4.5.1]: of CURRENT_TIME and CURRENT_TIMESTAMP, and implied by a time or timestamp without time zone.",
        ),
    ]
}

/// Settings of definitions and transactions.
fn definitions() -> [Definition; 5] {
    [
        defined(
            ("TABLES.ENGINE", Kind::Engine, text(crate::engine::rows::ROW_STORE)),
            DATABASE,
            "The storage engine of tables a schema creates.",
        ),
        defined(
            ("TABLES.UNIQUE_INDEX_METHOD", Kind::IndexMethod, text(crate::index::BTREE_METHOD)),
            DATABASE,
            "The index method behind the UNIQUE constraints of tables created from now on.",
        ),
        defined(
            ("TRANSACTION.STRICT", Kind::Choice(SWITCHES), text(ON)),
            (false, Effect::Transaction),
            "ON refuses an engine that falls short of serializable, recoverable transactions [X3.135-1986 §4.16]; OFF allows it.",
        ),
        defined(
            ("LOCKS.WAIT", Kind::Choice(SWITCHES), text(OFF)),
            (true, Effect::Transaction),
            "ON waits for a lock another transaction holds; OFF refuses the statement at once.",
        ),
        defined(
            ("SESSION.COMMIT_AT_END", Kind::Choice(SWITCHES), text(ON)),
            (true, Effect::Statement),
            "Whether a transaction open when its session ends is committed, ON, or rolled back, OFF [X3.135-1986 §7.1 GR2].",
        ),
    ]
}

/// The optimizer's passes and the physical planner's cost weights.
fn cost() -> [Definition; 3] {
    let weight = Kind::Number { min: 0.0_f64, max: f64::MAX };
    [
        defined(
            ("OPTIMIZER.MAX_PASSES", Kind::Integer { min: 1, max: 1_000_000 }, count(crate::optimizer::DEFAULT_MAX_PASSES)),
            DATABASE,
            "The most passes the optimizer makes over a plan.",
        ),
        defined(
            ("PLANNER.INDEX_DESCENT_COST", weight.clone(), Value::Approx(crate::physical::DEFAULT_INDEX_DESCENT)),
            DATABASE,
            "The cost of one level of an index descent, a row read in sequence costing 1.",
        ),
        defined(
            ("PLANNER.SORT_COMPARE_COST", weight, Value::Approx(crate::physical::DEFAULT_SORT_COMPARE)),
            DATABASE,
            "The cost of one comparison of a sort, a row read in sequence costing 1.",
        ),
    ]
}

/// The memory budget and operators' work areas.
fn memory() -> [Definition; 3] {
    [
        defined(
            ("MEMORY.BUDGET_BYTES", at_least(1_i128 << 20_i32), count(crate::memory::DEFAULT_BUDGET)),
            DATABASE,
            "The bytes the database's buffer pools, work areas and caches may reserve in all, with the files of a database kept in memory, its pages, temporary tables and spills; lowering it takes back none they hold.",
        ),
        defined(
            ("MEMORY.WORK_BYTES", at_least(64_i128 << 10_i32), count(crate::spill::DEFAULT_WORK_MEMORY)),
            SESSION,
            "The bytes a sort, hash join, grouping, DISTINCT, EXCEPT or INTERSECT holds in memory before it spills to a work area of as many, reserved of MEMORY.BUDGET_BYTES; so too the rows a cursor reads ahead of its fetches or keeps to scroll among, and those an INSERT, UPDATE or DELETE chooses before it changes any, and what it did, kept for the checks of its constraints, a deferred one's until made; and the pages of each of a session's instances of temporary tables, the rest written to a spill file.",
        ),
        defined(
            ("SPILL.BATCH_BYTES", Kind::Integer { min: 4_i128 << 10_i32, max: 1_i128 << 20_i32 }, count(crate::spill::DEFAULT_SPILL_BATCH)),
            SESSION,
            "The bytes of rows a spilling operator gathers before it writes them to its work area; it reads or writes a run for every two batches its MEMORY.WORK_BYTES holds.",
        ),
    ]
}

/// What the planner takes when nothing better is known, and when it
/// decorrelates.
fn guesses() -> [Definition; 5] {
    use crate::physical::{DEFAULT_EQUAL, DEFAULT_NULL, DEFAULT_RANGE, DEFAULT_REORDER_GAIN};
    [
        defined(
            ("PLANNER.EQUAL_SELECTIVITY", FRACTION, Value::Approx(DEFAULT_EQUAL)),
            DATABASE,
            "The fraction of rows an equality keeps when the statistics do not tell.",
        ),
        defined(
            ("PLANNER.RANGE_SELECTIVITY", FRACTION, Value::Approx(DEFAULT_RANGE)),
            DATABASE,
            "The fraction of rows an order comparison, a pattern or a subquery keeps when the statistics do not tell.",
        ),
        defined(
            ("PLANNER.NULL_FRACTION", FRACTION, Value::Approx(DEFAULT_NULL)),
            DATABASE,
            "The fraction of a column's values taken to be null when the statistics do not tell.",
        ),
        defined(
            ("PLANNER.REORDER_GAIN", Kind::Number { min: 0.0, max: 0.999 }, Value::Approx(DEFAULT_REORDER_GAIN)),
            DATABASE,
            "A join order is taken over the FROM clause's if its estimated cost is less than this fraction of that order's.",
        ),
        defined(
            ("EXECUTOR.DECORRELATE_AFTER_RUNS", Kind::Integer { min: 0, max: 1_000_000 }, count(crate::exec::DEFAULT_DECORRELATE_AFTER)),
            DATABASE,
            "The runs of a correlated subquery, for as many sets of values, after which it is run once for all of them.",
        ),
    ]
}

/// How a spilled operator is partitioned.
fn spilling() -> [Definition; 2] {
    use crate::exec::{DEFAULT_PARTITION_DEPTH, DEFAULT_PARTITIONS};
    [
        defined(
            ("SPILL.PARTITIONS", Kind::Integer { min: 2, max: 4096 }, count(DEFAULT_PARTITIONS)),
            SESSION,
            "The most partitions a spilled operator (a join, grouping, DISTINCT, EXCEPT or INTERSECT) writes at once; it writes fewer if its MEMORY.WORK_BYTES holds fewer runs.",
        ),
        defined(
            ("SPILL.PARTITION_DEPTH", Kind::Integer { min: 0, max: 64 }, count(DEFAULT_PARTITION_DEPTH)),
            SESSION,
            "The most times a spilled operator partitions a partition again, before it holds the partition in a table, or, grouping and an outer join, makes it in passes.",
        ),
    ]
}

/// When a database kept in a directory checkpoints and compacts.
fn durability() -> [Definition; 3] {
    use crate::db::{CHECKPOINT_AFTER, CHECKPOINT_BYTES, COMPACT_AFTER};
    [
        defined(
            ("CHECKPOINT.AFTER_CHANGES", at_least(1), count(CHECKPOINT_AFTER)),
            DATABASE,
            "The row changes logged after which the end of a transaction takes a checkpoint.",
        ),
        defined(
            ("CHECKPOINT.AFTER_BYTES", at_least(1), count(CHECKPOINT_BYTES)),
            DATABASE,
            "The bytes written to the data file since the last checkpoint after which the end of a transaction takes one.",
        ),
        defined(
            ("CATALOG.COMPACT_AFTER_IMAGES", Kind::Integer { min: 1, max: 1_000_000 }, count(COMPACT_AFTER)),
            DATABASE,
            "The images the catalog file holds after which the next is written, alone, to the other file.",
        ),
    ]
}
