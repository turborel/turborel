//! Filters moved into set operations and below window partitions, and
//! FETCH FIRST into the operands of UNION ALL (RULES.md D374,
//! `optimizer/setops.rs`, `windows.rs`, `limits.rs`).

use turborel::optimizer::{PUSH_FILTER_THROUGH_SET_OPERATION, PUSH_FILTER_THROUGH_WINDOW, PUSH_LIMIT_THROUGH_UNION};

use super::{alike, left, tables};

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR3b iii]; [DESIGN] RULES.md
/// D374: a filter over UNION ALL, UNION, INTERSECT and EXCEPT, with ALL
/// and without, of rows with duplicates and nulls, keeps the same rows
/// moved into the operands; one that can raise over UNION ALL and UNION
/// raises alike. LIKE, which can tell rows not distinct apart, over UNION,
/// one that can raise over INTERSECT and EXCEPT, and a set operation by a
/// collation's sort keys are left.
#[test]
fn filters_go_into_set_operations() {
    let (_db, s) = tables();
    let of = |op: &str, filter: &str| {
        format!("SELECT * FROM (SELECT K, V, S FROM {{T}} {op} SELECT K, V, S FROM {{U}}) X WHERE {filter} ORDER BY 1, 2, 3")
    };
    let filters = ["X.K > 1 OR X.K IS NULL", "X.S = 'a'", "X.V IS NULL AND X.K = 3"];
    for op in ["UNION ALL", "UNION", "INTERSECT", "INTERSECT ALL", "EXCEPT", "EXCEPT ALL"] {
        for filter in filters {
            alike(&s, PUSH_FILTER_THROUGH_SET_OPERATION, &of(op, filter));
        }
    }
    let raised = alike(&s, PUSH_FILTER_THROUGH_SET_OPERATION, &of("UNION ALL", "10 / X.K > 1"));
    assert!(raised.iter().all(|r| r == "Data"), "{raised:?}");
    alike(&s, PUSH_FILTER_THROUGH_SET_OPERATION, &of("UNION", "10 / X.K > 1"));
    alike(&s, PUSH_FILTER_THROUGH_SET_OPERATION, &of("UNION", "X.K + 1 = 2"));
    alike(&s, PUSH_FILTER_THROUGH_SET_OPERATION, &of("UNION ALL", "X.S LIKE 'a%'"));
    for (op, filter) in [("UNION", "X.S LIKE 'a%'"), ("INTERSECT", "10 / X.K > 1"), ("EXCEPT", "10 / X.K > 1")] {
        left(&s, PUSH_FILTER_THROUGH_SET_OPERATION, &of(op, filter));
    }
    let collated = "SELECT * FROM (SELECT S COLLATE CASE_INSENSITIVE AS S FROM {T} UNION SELECT S COLLATE CASE_INSENSITIVE AS S FROM {U}) X WHERE X.S = 'a'";
    left(&s, PUSH_FILTER_THROUGH_SET_OPERATION, collated);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.15 GR3]; [DESIGN] RULES.md D374: a
/// conjunct over a window's partitioning columns goes below it, of
/// ROW_NUMBER, RANK, aggregates over ROWS and RANGE frames, LAG and of
/// windows one over another, each function's value alike; one over
/// another column, and one beside a conjunct left above that can raise,
/// stay above.
#[test]
fn filters_go_below_window_partitions() {
    let (_db, s) = tables();
    let windowed = "SELECT * FROM (SELECT A.K, A.V, A.S,
          ROW_NUMBER() OVER (PARTITION BY A.K ORDER BY A.V, A.S) R,
          RANK() OVER (PARTITION BY A.K ORDER BY A.V) N,
          SUM(A.V) OVER (PARTITION BY A.K ORDER BY A.V ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) M,
          SUM(A.V) OVER (PARTITION BY A.K ORDER BY A.V RANGE BETWEEN 2 PRECEDING AND CURRENT ROW) G,
          LAG(A.V) OVER (PARTITION BY A.K ORDER BY A.V, A.S) P
        FROM {A} A) X WHERE {F} ORDER BY 1, 2, 3, 4";
    for filter in ["X.K = 7", "X.K < 20 OR X.K IS NULL", "X.K BETWEEN 3 AND 9 AND X.R > 1"] {
        alike(&s, PUSH_FILTER_THROUGH_WINDOW, &windowed.replace("{F}", filter));
    }
    let stacked = "SELECT * FROM (SELECT C.K, C.X, SUM(C.X) OVER (PARTITION BY C.K) T,
          COUNT(*) OVER (PARTITION BY C.K, C.X) U FROM {C} C) X WHERE X.K = 3 AND X.X > 0 ORDER BY 1, 2";
    alike(&s, PUSH_FILTER_THROUGH_WINDOW, stacked);
    for filter in ["X.V = 3", "X.K = 7 AND 10 / X.V > 1", "X.S LIKE 's1%'"] {
        left(&s, PUSH_FILTER_THROUGH_WINDOW, &windowed.replace("{F}", filter));
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR6, GR7]; [DESIGN] RULES.md
/// D374: FETCH FIRST over UNION ALL, sorted or not, with OFFSET or
/// without, gives each operand the rows it needs, and the same rows;
/// WITH TIES, PERCENT and a FETCH FIRST over UNION are left.
#[test]
fn limits_go_into_union_all() {
    let (_db, s) = tables();
    let union = "SELECT K, V FROM {A} UNION ALL SELECT K, W FROM {B}";
    for tail in [
        "ORDER BY 1, 2 FETCH FIRST 5 ROWS ONLY",
        "ORDER BY 1 DESC NULLS FIRST, 2 OFFSET 7 ROWS FETCH FIRST 4 ROWS ONLY",
        "ORDER BY 2, 1 FETCH FIRST 700 ROWS ONLY",
        "FETCH FIRST 3 ROWS ONLY",
        "OFFSET 598 ROWS FETCH FIRST 4 ROWS ONLY",
    ] {
        alike(&s, PUSH_LIMIT_THROUGH_UNION, &format!("{union} {tail}"));
    }
    let nested = "SELECT K FROM {C} UNION ALL SELECT K FROM {D} UNION ALL SELECT K FROM {Z} ORDER BY 1 FETCH FIRST 6 ROWS ONLY";
    alike(&s, PUSH_LIMIT_THROUGH_UNION, nested);
    for tail in ["ORDER BY 1 FETCH FIRST 5 ROWS WITH TIES", "ORDER BY 1, 2 FETCH FIRST 10 PERCENT ROWS ONLY", "ORDER BY 1, 2 OFFSET 3 ROWS"]
    {
        left(&s, PUSH_LIMIT_THROUGH_UNION, &format!("{union} {tail}"));
    }
    left(&s, PUSH_LIMIT_THROUGH_UNION, "SELECT K, V FROM {A} UNION SELECT K, W FROM {B} ORDER BY 1, 2 FETCH FIRST 5 ROWS ONLY");
}
