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
