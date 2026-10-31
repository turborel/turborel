//! Unit tests of the settings registry: its kinds and scopes, and the
//! built-in keys (RULES.md D36, R-TEST-1). Split from settings.rs (RULES.md
//! R-SIZE-2).

use super::builtin::{LOG_DURABILITY, text};
use super::*;
use crate::txn::LockWait;
use crate::value::Decimal;

/// Each built-in key and its setting's name, in the order of the keys.
const BUILTIN: [(Key, &str); 42] = [
    (TABLES_ENGINE, "TABLES.ENGINE"),
    (TABLES_UNIQUE_INDEX_METHOD, "TABLES.UNIQUE_INDEX_METHOD"),
    (TRANSACTION_STRICT, "TRANSACTION.STRICT"),
    (LOCKS_WAIT, "LOCKS.WAIT"),
    (SESSION_COMMIT_AT_END, "SESSION.COMMIT_AT_END"),
    (OPTIMIZER_MAX_PASSES, "OPTIMIZER.MAX_PASSES"),
    (INDEX_DESCENT_COST, "PLANNER.INDEX_DESCENT_COST"),
    (SORT_COMPARE_COST, "PLANNER.SORT_COMPARE_COST"),
    (CATALOG_NAME, "CATALOG.NAME"),
    (MEMORY_BUDGET_BYTES, "MEMORY.BUDGET_BYTES"),
    (MEMORY_WORK_BYTES, "MEMORY.WORK_BYTES"),
    (SPILL_BATCH_BYTES, "SPILL.BATCH_BYTES"),
    (EQUAL_SELECTIVITY, "PLANNER.EQUAL_SELECTIVITY"),
    (RANGE_SELECTIVITY, "PLANNER.RANGE_SELECTIVITY"),
    (NULL_FRACTION, "PLANNER.NULL_FRACTION"),
    (REORDER_GAIN, "PLANNER.REORDER_GAIN"),
    (DECORRELATE_AFTER_RUNS, "EXECUTOR.DECORRELATE_AFTER_RUNS"),
    (EXECUTOR_BATCHES, "EXECUTOR.BATCHES"),
    (SPILL_PARTITIONS, "SPILL.PARTITIONS"),
    (SPILL_PARTITION_DEPTH, "SPILL.PARTITION_DEPTH"),
    (CHECKPOINT_AFTER_CHANGES, "CHECKPOINT.AFTER_CHANGES"),
    (CHECKPOINT_AFTER_BYTES, "CHECKPOINT.AFTER_BYTES"),
    (CATALOG_COMPACT_AFTER_IMAGES, "CATALOG.COMPACT_AFTER_IMAGES"),
    (PAGE_BYTES, "STORAGE.PAGE_BYTES"),
    (CHECKSUM, "STORAGE.CHECKSUM"),
    (ENCODINGS, "STORAGE.ENCODINGS"),
    (LOG_SEGMENT_BYTES, "LOG.SEGMENT_BYTES"),
    (POOL_BYTES, "STORAGE.POOL_BYTES"),
    (STATEMENT_TIMEOUT_MS, "STATEMENT.TIMEOUT_MS"),
    (LOCKS_TIMEOUT_MS, "LOCKS.TIMEOUT_MS"),
    (SLOW_STATEMENT_MS, "DIAGNOSTICS.SLOW_STATEMENT_MS"),
    (SESSION_TIME_ZONE, "SESSION.TIME_ZONE"),
    (JOBS_POLICY, "JOBS.POLICY"),
    (FILES_MAX_OPEN, "FILES.MAX_OPEN"),
    (DIAGNOSTICS_LEVEL, "DIAGNOSTICS.LEVEL"),
    (SPILL_ENCODINGS, "SPILL.ENCODINGS"),
    (COMMIT_SYNC, "LOG.COMMIT_SYNC"),
    (FUNCTION_CALLS, "SQL.FUNCTION_CALLS"),
    (CHECKPOINT_OVERDUE_FACTOR, "CHECKPOINT.OVERDUE_FACTOR"),
    (LOG_SYNC_AFTER_MS, "LOG.SYNC_AFTER_MS"),
    (READ_AHEAD_PAGES, "STORAGE.READ_AHEAD_PAGES"),
    (LOG_DURABILITY, "LOG.DURABILITY"),
];

/// Verifies: [DESIGN] RULES.md D36: each built-in key is its setting's
/// place, and each default is of its kind.
#[test]
fn builtin_keys_are_their_settings() {
    let s = Settings::default();
    assert_eq!(s.list(&Own::default()).len(), BUILTIN.len());
    for (key, name) in BUILTIN {
        assert_eq!(s.key(name), Some(key), "{name}");
        let d = s.definition(key).unwrap();
        assert_eq!(accepted(&d.name, &d.kind, &d.default).ok().map(|v| v.to_literal()), Some(d.default.to_literal()), "{name}");
    }
}

/// Verifies: [DESIGN] RULES.md D36, D42: a text setting takes text up
/// to its length, its ends trimmed, an empty one too; not longer text,
/// nor a number.
#[test]
fn text_is_taken_up_to_its_length() {
    let shown = |r: Result<Value>| r.ok().map(|v| v.to_literal());
    let path = Kind::Text { max: 4 };
    assert_eq!(shown(accepted("X", &path, &text(" a/b "))), Some("'a/b'".to_owned()));
    assert_eq!(shown(accepted("X", &path, &text(""))), Some("''".to_owned()), "empty");
    assert!(accepted("X", &path, &text("a/b/c")).is_err(), "too long");
    assert!(accepted("X", &path, &whole(1)).is_err(), "not text");
}

/// Verifies: [EXTENSION] RULES.md D41 [ISO/IEC 9075:1992 §5.2 SR10,
/// SR13, SR14]: a list of columns takes names by commas, a regular one
/// upper case, a delimited one as written, its doubled quotes one, each
/// written back delimited only if it must be; none, empty; not more
/// than its most, nor a name empty, nor a quote not closed.
#[test]
fn columns_are_taken_as_identifiers() {
    let shown = |r: Result<Value>| r.ok().map(|v| v.to_literal());
    let two = Kind::Columns { most: 2 };
    assert_eq!(shown(accepted("X", &two, &text(" a ,  \"b\"\"c\" "))), Some("'A, \"b\"\"c\"'".to_owned()));
    assert_eq!(shown(accepted("X", &two, &text("\"B_1\", x"))), Some("'B_1, X'".to_owned()), "delimited only if it must be");
    assert_eq!(shown(accepted("X", &two, &text("  "))), Some("''".to_owned()), "none");
    for refused in ["a, b, c", "a,", ",a", "a,,b", "\"a", "\"a\" b", "\"\""] {
        assert!(accepted("X", &two, &text(refused)).is_err(), "{refused}");
    }
    assert!(accepted("X", &two, &whole(1)).is_err(), "not text");
}

/// Verifies: [DESIGN] RULES.md D36: a value is taken only of its
/// setting's kind and range: a whole number as one, a number of either
/// kind, a word in any case; a value set before its setting is declared
/// is its value once it is; a session sets only a setting of its own.
#[test]
fn values_are_taken_of_their_kind() {
    let range = Kind::Integer { min: 1, max: 10 };
    let shown = |r: Result<Value>| r.ok().map(|v| v.to_literal());
    assert_eq!(shown(accepted("X", &range, &Value::Exact(Decimal::new(500, 2)))), Some("5".to_owned()));
    for refused in [Value::Exact(Decimal::new(55, 1)), whole(11), whole(0), text("5")] {
        assert!(accepted("X", &range, &refused).is_err(), "{refused}");
    }
    let number = Kind::Number { min: 0.0_f64, max: 1.0_f64 };
    assert_eq!(shown(accepted("X", &number, &Value::Exact(Decimal::new(5_i128, 1_u32)))), Some(Value::Approx(0.5).to_literal()));
    assert!(accepted("X", &number, &Value::Approx(f64::NAN)).is_err());
    assert_eq!(shown(accepted("X", &Kind::Choice(SWITCHES), &text(" on "))), Some("'ON'".to_owned()));
    assert!(accepted("X", &Kind::Choice(SWITCHES), &text("SOMETIMES")).is_err());

    let mut s = Settings::default();
    s.set("plugin.depth", Some(&whole(3))).unwrap();
    let declared = Definition {
        name: "Plugin.Depth".into(),
        kind: range,
        default: whole(1),
        per_session: false,
        node: false,
        effect: Effect::Statement,
        doc: String::new(),
    };
    let key = s.declare(declared.clone()).unwrap();
    assert_eq!((s.integer(key), s.effective(key, &Own::default()).1), (3, Source::Database));
    assert!(s.declare(declared).is_err(), "twice");
    s.set("PLUGIN.DEPTH", None).unwrap();
    assert_eq!((s.integer(key), s.effective(key, &Own::default()).1), (1, Source::Default));

    let mut own = Own::default();
    own.set(&s, "locks.wait", Some(&text("on"))).unwrap();
    assert_eq!(lock_wait(s.effective(LOCKS_WAIT, &own).0), LockWait::Wait);
    assert!(own.set(&s, "PLUGIN.DEPTH", Some(&whole(2))).is_err(), "the database's");
    assert!(own.set(&s, "NO.SUCH", None).is_err());
    own.set(&s, "LOCKS.WAIT", None).unwrap();
    assert_eq!(s.effective(LOCKS_WAIT, &own).1, Source::Default);
}
