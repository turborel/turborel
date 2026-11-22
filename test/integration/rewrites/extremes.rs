//! MIN and MAX read from the first row in order (RULES.md D413,
//! `optimizer/extremes.rs`).

use turborel::optimizer::EXTREMES_BY_ORDER;

use turborel::db::{Database, Session};

use super::{COLUMNS, alike, left, plan, tables};

/// Schema Q's tables, and N of each store: A's rows with no null, its
/// columns NOT NULL, and an index on V and K.
fn with_n() -> (Database, Session) {
    let (db, s) = tables();
    for (prefix, store) in [("", ""), (COLUMNS, " USING columnstore")] {
        s.run(&format!("CREATE TABLE {prefix}N (K INTEGER NOT NULL, V INTEGER NOT NULL, S VARCHAR(10) NOT NULL){store}")).unwrap();
        s.run(&format!("INSERT INTO {prefix}N SELECT COALESCE(K, -1), V, S FROM A")).unwrap();
        s.commit().unwrap();
        db.create_index(&format!("{prefix}N_VK"), &format!("Q.{prefix}N"), &["V", "K"], "btree").unwrap();
    }
    db.maintain().unwrap();
    (db, s)
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7a, GR7d iv]; [DESIGN] RULES.md
/// D413: MIN and MAX of an exact numeric column known not null, over a
/// table filtered or not and of no rows, and MIN DISTINCT, give the same
/// value read from the first row in order, of either store; over an index
/// whose first columns the filter fixes and whose next is the column, the
/// read stops at that row. A column that may hold a null, whose nulls
/// eliminated warn, two set functions, a grouping, a character column, an
/// expression, a filter clause, and a filter that can raise are left.
#[test]
fn extremes_are_read_from_the_first_row() {
    let (_db, s) = with_n();
    for sql in [
        "SELECT MIN(K) FROM {N}",
        "SELECT MAX(K) FROM {N} WHERE V = 3",
        "SELECT MIN(K) FROM {N} WHERE V = 3 AND K > 100",
        "SELECT MAX(V) FROM {N} WHERE K < 100",
        "SELECT MIN(K) FROM {N} WHERE V = 1000",
        "SELECT MIN(DISTINCT V) FROM {N} WHERE S LIKE 's1%'",
    ] {
        alike(&s, EXTREMES_BY_ORDER, sql);
    }
    for sql in ["SELECT MAX(K) FROM N WHERE V = 3", "SELECT MIN(K) FROM N WHERE V = 2 AND K > 40"] {
        let shown = plan(&s, sql);
        let physical = shown.split("-- physical:").nth(1).unwrap_or_default();
        assert!(physical.contains("Limit fetch 1") && physical.contains("N_VK"), "{sql}: {shown}");
        assert!(!physical.contains("Sort"), "{sql}: {shown}");
    }
    for sql in [
        "SELECT MIN(K) FROM {A}",
        "SELECT MAX(K) FROM {E}",
        "SELECT MIN(K), MAX(K) FROM {N}",
        "SELECT V, MIN(K) FROM {N} GROUP BY V",
        "SELECT MIN(S) FROM {N}",
        "SELECT MAX(K + V) FROM {N}",
        "SELECT MIN(K) FILTER (WHERE V > 2) FROM {N}",
    ] {
        left(&s, EXTREMES_BY_ORDER, sql);
    }
    let raised = left(&s, EXTREMES_BY_ORDER, "SELECT MIN(K) FROM {N} WHERE 10 / (V - 3) > 1");
    assert!(raised.iter().all(|r| r == "Data"), "{raised:?}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7a, GR7d iv]; [DESIGN] RULES.md
/// D413: MIN and MAX of a column that may hold a null are read from the
/// first row in order where a conjunct of the filter over that column
/// alone is not true of its null, so no null is eliminated and no warning
/// raised, of rows found or of none, by the column's index; a filter that
/// keeps a null, or whose conjunct reads another column too, is left, its
/// warning as it was.
#[test]
fn extremes_of_a_column_whose_nulls_the_filter_rejects() {
    let (_db, s) = with_n();
    for sql in [
        "SELECT MIN(K) FROM {A} WHERE K > 10",
        "SELECT MAX(K) FROM {A} WHERE K IS NOT NULL AND V = 3",
        "SELECT MIN(K) FROM {A} WHERE V = 2 AND K BETWEEN 400 AND 500",
        "SELECT MAX(K) FROM {T} WHERE K = K",
    ] {
        alike(&s, EXTREMES_BY_ORDER, sql);
    }
    let shown = plan(&s, "SELECT MIN(K) FROM A WHERE K > 10");
    let physical = shown.split("-- physical:").nth(1).unwrap_or_default();
    assert!(physical.contains("Limit fetch 1") && !physical.contains("Sort"), "{shown}");
    for sql in [
        "SELECT MIN(K) FROM {A} WHERE V > 3",
        "SELECT MIN(K) FROM {A} WHERE K > 10 OR V = 1",
        "SELECT MAX(K) FROM {A} WHERE K IS NULL",
        "SELECT MIN(K) FROM {A} WHERE COALESCE(K, 0) >= 0",
    ] {
        left(&s, EXTREMES_BY_ORDER, sql);
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7d iv]; [DESIGN] RULES.md D413:
/// MIN and MAX of a date or timestamp without time zone are read from the
/// first row in order, of either store, nulls rejected or none held; an
/// approximate number, whose zeros alike differ by sign, is left.
#[test]
fn extremes_of_datetimes_are_read_from_the_first_row() {
    let (_db, s) = tables();
    for (prefix, store) in [("", ""), (COLUMNS, " USING columnstore")] {
        s.run(&format!("CREATE TABLE {prefix}W (D DATE NOT NULL, TS TIMESTAMP(0), R REAL NOT NULL){store}")).unwrap();
        let rows: Vec<String> = (1..=60)
            .map(|i| {
                let ts = if i % 7 == 0 { "NULL".to_owned() } else { format!("TIMESTAMP '2024-03-{:02} 10:{:02}:00'", i % 28 + 1, i % 60) };
                format!("(DATE '2024-01-01' + INTERVAL '{}' DAY, {ts}, {})", (i * 37) % 60, f64::from(i) - 30.0)
            })
            .collect();
        s.run(&format!("INSERT INTO {prefix}W VALUES {}", rows.join(", "))).unwrap();
        s.commit().unwrap();
    }
    for sql in [
        "SELECT MIN(D) FROM {W}",
        "SELECT MAX(D) FROM {W} WHERE R > 0",
        "SELECT MAX(TS) FROM {W} WHERE TS > TIMESTAMP '2024-03-05 00:00:00'",
        "SELECT MIN(TS) FROM {W} WHERE TS < TIMESTAMP '2000-01-01 00:00:00'",
    ] {
        alike(&s, EXTREMES_BY_ORDER, sql);
    }
    for sql in ["SELECT MIN(TS) FROM {W}", "SELECT MIN(R) FROM {W}"] {
        left(&s, EXTREMES_BY_ORDER, sql);
    }
}
