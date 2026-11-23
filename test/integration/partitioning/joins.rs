//! Index nested-loop joins into a partitioned table, each probe routed to
//! the partitions its values may be in.

use turborel::db::Session;
use turborel::value::Value;

use crate::common::{exec, sorted};

use super::{plan, sales};

/// SALES of 600 rows, 200 of each year, ID unique, and WANTED, of three
/// rows naming three of them by ID and SOLD, and one naming none.
fn joined() -> (turborel::db::Database, Session) {
    let (db, s) = sales();
    for i in 0..600 {
        let year = 2025 + i % 3;
        exec(&s, &format!("INSERT INTO SALES VALUES ({i}, DATE '{year}-02-01', {i})")).unwrap();
    }
    exec(&s, "CREATE TABLE WANTED (ID INTEGER NOT NULL, SOLD DATE NOT NULL)").unwrap();
    exec(&s, "INSERT INTO WANTED VALUES (3, DATE '2025-02-01'), (4, DATE '2026-02-01'), (5, DATE '2027-02-01'), (9, DATE '2026-02-01')")
        .unwrap();
    exec(&s, "COMMIT").unwrap();
    (db, s)
}

/// Verifies: [EXTENSION] RULES.md D156: a join on the partition key and an
/// index's columns probes the index of each partition, each probe routed,
/// as its values are bound, to the one partition that may hold its rows;
/// EXPLAIN tells so; a LEFT join keeps a row no partition matches with
/// nulls.
#[test]
fn a_join_probes_the_partition_each_row_routes_to() {
    let (_db, s) = joined();
    let sql = "SELECT W.ID, S.AMOUNT FROM WANTED W JOIN SALES S ON S.ID = W.ID AND S.SOLD = W.SOLD";
    let shown = plan(&s, sql);
    assert!(shown.contains("IndexNestedLoop") && shown.contains("each probe routed to one"), "{shown}");
    assert!(!shown.contains("S2025"), "{shown}");
    assert_eq!(sorted(&s, sql), ["3,3.00", "4,4.00", "5,5.00"]);
    let left = "SELECT W.ID, S.AMOUNT FROM WANTED W LEFT JOIN SALES S ON S.ID = W.ID AND S.SOLD = W.SOLD";
    let shown = plan(&s, left);
    assert!(shown.contains("LeftOuterIndexNestedLoop") || shown.contains("IndexNestedLoop"), "{shown}");
    assert_eq!(sorted(&s, left), ["3,3.00", "4,4.00", "5,5.00", "9,NULL"]);
}

/// Verifies: [EXTENSION] RULES.md D156, D157: a join on columns that fix
/// no partition key probes every partition's index, EXPLAIN telling how
/// many; given a global index of them, it probes that, once a row.
#[test]
fn a_join_on_other_columns_probes_each_partition_or_a_global_index() {
    let (_db, s) = joined();
    let sql = "SELECT W.ID, S.SOLD FROM WANTED W JOIN SALES S ON S.ID = W.ID";
    let shown = plan(&s, sql);
    assert!(shown.contains("IndexNestedLoop") && shown.contains("each probe of 3"), "{shown}");
    let rows = ["3,2025-02-01", "4,2026-02-01", "5,2027-02-01", "9,2025-02-01"];
    assert_eq!(sorted(&s, sql), rows);
    exec(&s, "ALTER TABLE SALES ADD CONSTRAINT ONE_ID UNIQUE (ID)").unwrap();
    exec(&s, "COMMIT").unwrap();
    let shown = plan(&s, sql);
    assert!(shown.contains("IndexNestedLoop") && shown.contains("GLOBAL"), "{shown}");
    assert_eq!(sorted(&s, sql), rows);
}

/// Verifies: [EXTENSION] RULES.md D156: a prepared join, planned once,
/// routes each probe by the values of each execution's rows, the rows of
/// each execution its own; the rows of a collection derived table read
/// laterally of the table before it are joined, and a subquery of the
/// outer row's values reads the partitions those values admit.
#[test]
fn prepared_and_lateral_joins_route_each_probe() {
    let (_db, s) = joined();
    let sql = "SELECT S.AMOUNT FROM WANTED W JOIN SALES S ON S.ID = W.ID AND S.SOLD = W.SOLD WHERE W.ID = CAST(? AS INTEGER)";
    let shown = plan(&s, &sql.replace("CAST(? AS INTEGER)", "3"));
    assert!(shown.contains("each probe routed to one"), "{shown}");
    let p = s.prepare(sql).unwrap();
    for (id, want) in [("3", "3.00"), ("5", "5.00"), ("9", ""), ("4", "4.00")] {
        let mut c = s.open_prepared(&p, &[Value::Char(id.into())]).unwrap();
        let got = c.fetch().unwrap().map(|r| r[0].to_string()).unwrap_or_default();
        assert_eq!(got, want, "{id}");
    }
    let lateral = "SELECT U.K, S.AMOUNT FROM WANTED W, UNNEST(ARRAY[W.ID, W.ID + 300]) AS U (K), SALES S \
                   WHERE S.ID = U.K AND S.SOLD = W.SOLD AND W.ID < 6";
    assert_eq!(sorted(&s, lateral), ["3,3.00", "303,303.00", "304,304.00", "305,305.00", "4,4.00", "5,5.00"]);
    // A subquery of the outer row's values is read with them, each pruned.
    let correlated = "SELECT W.ID, (SELECT S.AMOUNT FROM SALES S WHERE S.ID = W.ID AND S.SOLD = W.SOLD) FROM WANTED W";
    assert_eq!(sorted(&s, correlated), ["3,3.00", "4,4.00", "5,5.00", "9,NULL"]);
}
