//! COUNT of a value never null counts rows (RULES.md D1172,
//! `optimizer/counted_rows.rs`).

use turborel::optimizer::{COUNT_ROWS_OF_VALUES, DISTINCT_AGGREGATES};

use turborel::db::Session;

use super::{COLUMNS, alike, left, plan, rule, tables};

/// Schema Q's tables, and N of each store: A's rows with no null, its
/// columns NOT NULL.
fn with_n() -> Session {
    let (db, s) = tables();
    for (prefix, store) in [("", ""), (COLUMNS, " USING columnstore")] {
        s.run(&format!("CREATE TABLE {prefix}N (K INTEGER NOT NULL, V INTEGER NOT NULL, S VARCHAR(10) NOT NULL){store}")).unwrap();
        s.run(&format!("INSERT INTO {prefix}N SELECT COALESCE(K, -1), V, S FROM A")).unwrap();
        s.commit().unwrap();
    }
    db.maintain().unwrap();
    s
}

/// The physical plan EXPLAIN shows of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    plan(s, sql).split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// Verifies: [X3.135-1986 §5.8 GR1, GR4]; [DESIGN] RULES.md D1172: COUNT,
/// not DISTINCT, of a column declared NOT NULL, through a filter, a
/// grouping and an inner join, with its filter clause, and of a literal
/// that is not null, gives the count of rows, alike with the rule off, of
/// either store, read by batches and row by row, of rows found or of
/// none; and a bare count of a table is then counted by its storage. A
/// column that may hold a null, whose nulls taken out warn, one a full
/// outer join may make null, COUNT DISTINCT, made a grouping first or
/// not, an expression and another set function are left.
#[test]
fn counts_of_values_never_null_count_rows() {
    let s = with_n();
    for sql in [
        "SELECT COUNT(K) FROM {N}",
        "SELECT COUNT(K), SUM(V), COUNT(*) FROM {N} WHERE V > 2",
        "SELECT V, COUNT(S) FROM {N} GROUP BY V",
        "SELECT COUNT(1) FROM {A}",
        "SELECT COUNT(K) FILTER (WHERE V = 3), COUNT(S) FROM {N}",
        "SELECT COUNT(K) FROM {N} WHERE V = 1000",
        "SELECT COUNT(X.S) FROM {N} X JOIN {D} Y ON X.V = Y.K",
    ] {
        alike(&s, COUNT_ROWS_OF_VALUES, sql);
    }
    for prefix in ["", COLUMNS] {
        let counted = physical(&s, &format!("SELECT COUNT(K) FROM {prefix}N"));
        assert!(counted.contains(&format!("-- counted by storage: Q.{prefix}N")), "{counted}");
    }
    for sql in [
        "SELECT COUNT(K) FROM {A}",
        "SELECT COUNT(Y.K) FROM {D} X FULL JOIN {N} Y ON X.K = Y.V",
        "SELECT COUNT(DISTINCT K) FROM {N}",
        "SELECT COUNT(K + V) FROM {N}",
        "SELECT SUM(K), MAX(S) FROM {N}",
    ] {
        left(&s, COUNT_ROWS_OF_VALUES, sql);
    }
    // Of COUNT DISTINCT not made a grouping first.
    rule(&s, DISTINCT_AGGREGATES, false);
    left(&s, COUNT_ROWS_OF_VALUES, "SELECT COUNT(DISTINCT K), COUNT(DISTINCT S) FROM {N}");
    rule(&s, DISTINCT_AGGREGATES, true);
    let warned = left(&s, COUNT_ROWS_OF_VALUES, "SELECT COUNT(K) FROM {A}");
    assert!(warned.iter().all(|w| w.contains("Some")), "{warned:?}");
}
