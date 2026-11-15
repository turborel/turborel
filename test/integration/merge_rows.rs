//! The rows a MERGE writes [ISO/IEC 9075-2:2023 §14.12 GR6]: the
//! constraints they are checked against, immediate and deferred, and the
//! referential actions they fire; many rows past the work area; the same
//! rows by batches and row by row, of either layout, with and without an
//! index on the key; and cursors open on the target [GR2, GR3].

use std::ops::RangeInclusive;

use turborel::db::{Completion, Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::{Counted, failure, rows, sorted};
use crate::merge::{BEFORE, UPSERT, merged, state, statement, target};

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 5 C I 2, GR6a ii 8 A I;
/// §15.11; §15.14; X3.135-1986 §3.3] the rows a MERGE writes are checked
/// against the table's constraints once all are written: two rows
/// inserted with one key, a key updated onto another, a key updated onto
/// one inserted, a key left null, each a violation (SQLSTATE 23000) undoing
/// the statement; keys passing through each other, and a key deleted and
/// inserted again, are none.
#[test]
fn constraints_checked_once_every_row_is_written_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("INSERT INTO S VALUES (4, 1, 'X')").unwrap();
    assert_eq!(failure(a.run(UPSERT)), (Code::Constraint, "23000".to_owned()), "two rows of key 4");
    assert_eq!(target(&a), BEFORE);
    a.rollback().unwrap();
    for (sql, what) in [
        ("MERGE INTO T USING S ON T.K = S.K WHEN MATCHED THEN UPDATE SET K = 1", "onto key 1"),
        (
            "MERGE INTO T USING S ON T.K = S.K WHEN MATCHED THEN UPDATE SET K = T.K + 1 WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V, S.N)",
            "row 3 onto the 4 inserted",
        ),
        ("MERGE INTO T USING S ON T.K = S.K WHEN NOT MATCHED THEN INSERT (V) VALUES (S.V)", "a null key"),
    ] {
        assert_eq!(failure(a.run(sql)), (Code::Constraint, "23000".to_owned()), "{what}");
        assert_eq!(target(&a), BEFORE, "{what}");
    }
    let through = "MERGE INTO T USING S ON T.K = S.K \
                   WHEN MATCHED THEN UPDATE SET K = T.K + 1 \
                   WHEN NOT MATCHED THEN INSERT VALUES (S.K + 1, S.V, S.N)";
    assert_eq!(a.run(through).unwrap(), Completion::Success);
    assert_eq!(target(&a), ["1,10,a", "3,20,b", "4,30,c", "5,400,D"], "keys through each other");
    a.rollback().unwrap();
    let again = "MERGE INTO T USING S ON T.K = S.K \
                 WHEN MATCHED AND T.K = 3 THEN DELETE \
                 WHEN NOT MATCHED THEN INSERT VALUES (S.K - 1, S.V, S.N)";
    assert_eq!(a.run(again).unwrap(), Completion::Success);
    assert_eq!(target(&a), ["1,10,a", "2,20,b", "3,400,D"], "key 3 deleted, then inserted");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a i 7 B I 2; §15.8; §11.8]
/// rows a MERGE deletes fire ON DELETE CASCADE, deleting the rows that
/// reference them; a foreign key of no action that references one refuses
/// the deletion (SQLSTATE 23000), and a row a MERGE inserts referencing no
/// row is refused too, each undoing the statement.
#[test]
fn referential_actions_fired_by_merge_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("CREATE TABLE C (I INTEGER, K INTEGER REFERENCES T ON DELETE CASCADE)").unwrap();
    a.run("CREATE TABLE R (K INTEGER REFERENCES T)").unwrap();
    a.run("INSERT INTO C VALUES (1, 1), (2, 2), (3, 3)").unwrap();
    a.commit().unwrap();
    a.run("MERGE INTO T USING S ON T.K = S.K WHEN MATCHED THEN DELETE").unwrap();
    assert_eq!(target(&a), ["1,10,a"]);
    assert_eq!(rows(&a, "SELECT I FROM C").unwrap(), ["1"], "cascaded");
    a.rollback().unwrap();
    a.run("INSERT INTO R VALUES (3)").unwrap();
    a.commit().unwrap();
    let delete = "MERGE INTO T USING S ON T.K = S.K WHEN MATCHED THEN DELETE";
    assert_eq!(failure(a.run(delete)), (Code::Constraint, "23000".to_owned()), "R references row 3");
    assert_eq!(target(&a), BEFORE);
    assert_eq!(rows(&a, "SELECT I FROM C ORDER BY I").unwrap(), ["1", "2", "3"], "none cascaded");
    let orphan = "MERGE INTO C USING S ON C.K = S.K WHEN NOT MATCHED THEN INSERT VALUES (S.K * 10, S.K)";
    assert_eq!(failure(a.run(orphan)), (Code::Constraint, "23000".to_owned()), "no row 4 of T");
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM C").unwrap(), ["3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6; §4.25.2; ISO/IEC 9075:1992
/// §10.6 GR4, §14.3 GR4] a deferred constraint a MERGE breaks waits for
/// COMMIT. A later MERGE may put it right, and the commit succeeds; left
/// broken, the commit fails (SQLSTATE 40002), the transaction rolled back.
#[test]
fn deferred_constraints_checked_at_commit_14_12() {
    let db = merged();
    let a = db.session("A");
    a.run("CREATE TABLE U (K INTEGER CONSTRAINT UK UNIQUE DEFERRABLE INITIALLY DEFERRED, V INTEGER)").unwrap();
    a.commit().unwrap();
    assert_eq!(state(&a, "MERGE INTO U USING S ON U.V = S.V WHEN NOT MATCHED THEN INSERT VALUES (1, S.V)"), "00000", "three of key 1");
    a.run("MERGE INTO U USING S ON U.V = S.V WHEN MATCHED AND S.K > 2 THEN UPDATE SET K = S.K").unwrap();
    assert_eq!(state(&a, "COMMIT"), "00000", "put right");
    assert_eq!(rows(&a, "SELECT K, V FROM U ORDER BY V").unwrap(), ["1,200", "3,300", "4,400"]);
    assert_eq!(state(&a, "MERGE INTO U USING S ON U.V = S.V WHEN MATCHED THEN UPDATE SET K = 9"), "00000");
    assert_eq!(state(&a, "COMMIT"), "40002", "left broken");
    assert_eq!(rows(&a, "SELECT K, V FROM U ORDER BY V").unwrap(), ["1,200", "3,300", "4,400"], "rolled back");
}

/// Inserts into `table` of `s` a row (k, value(k)) for each k of `keys`,
/// 500 to a statement.
fn fill(s: &Session, table: &str, keys: RangeInclusive<i64>, value: impl Fn(i64) -> i64) {
    let all: Vec<i64> = keys.collect();
    for chunk in all.chunks(500) {
        let values: Vec<String> = chunk.iter().map(|&k| format!("({k}, {})", value(k))).collect();
        s.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
}

/// BIG's keys 1 to 3000 matched by SRC's 1501 to 4500, past 2500 deleted,
/// the others updated, SRC's past 3000 inserted.
const LARGE: &str = "MERGE INTO BIG USING SRC ON BIG.K = SRC.K \
     WHEN MATCHED AND SRC.K > 2500 THEN DELETE \
     WHEN MATCHED THEN UPDATE SET V = BIG.V + SRC.V \
     WHEN NOT MATCHED THEN INSERT VALUES (SRC.K, - SRC.V)";

/// COUNT(*), SUM(V) and SUM(K) of BIG after `LARGE`.
fn large_expected() -> String {
    let left: Vec<(i64, i64)> = (1..=4500_i64)
        .filter_map(|k| match k {
            1..=1500 => Some((k, 0)),
            1501..=2500 => Some((k, k)),
            2501..=3000 => None,
            _ => Some((k, -k)),
        })
        .collect();
    let (values, keys): (i64, i64) = (left.iter().map(|r| r.1).sum(), left.iter().map(|r| r.0).sum());
    format!("{},{values},{keys}", left.len())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR5, GR6a i 1 B II] RULES.md D38 a
/// MERGE of thousands of rows past MEMORY.WORK_BYTES, the rows it joins
/// and chooses held in spill areas, deletes, updates and inserts each row
/// as it would in memory, ROW_COUNT their number; two source rows for one
/// target row among them still raise 21000.
#[test]
fn a_large_merge_past_the_work_area_14_12() {
    let db = Database::new();
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE BIG (K INTEGER NOT NULL, V INTEGER) CREATE TABLE SRC (K INTEGER, V INTEGER)")
        .unwrap();
    let a = db.session("A");
    fill(&a, "BIG", 1..=3000, |_| 0);
    fill(&a, "SRC", 1501..=4500, |k| k);
    a.commit().unwrap();
    a.run("SET MEMORY.WORK_BYTES = 65536").unwrap();
    counted.taken();
    assert_eq!(a.run(LARGE).unwrap(), Completion::Success);
    assert!(counted.taken() > 0, "spilled past the work area");
    assert_eq!(statement(&a, "ROW_COUNT"), "3000", "500 deleted, 1000 updated, 1500 inserted");
    assert_eq!(rows(&a, "SELECT COUNT(*), SUM(V), SUM(K) FROM BIG").unwrap(), [large_expected()]);
    a.rollback().unwrap();
    a.run("INSERT INTO SRC VALUES (2999, 1)").unwrap();
    assert_eq!(failure(a.run(LARGE)), (Code::Cardinality, "21000".to_owned()), "key 2999 twice");
    assert_eq!(rows(&a, "SELECT COUNT(*), SUM(V) FROM BIG").unwrap(), ["3000,0"]);
    a.rollback().unwrap();
}

/// The MERGEs `alike` runs, on T of keys 1 to 40 and S of keys 20 to 60
/// and a null key.
const ALIKE: [&str; 2] = [
    "MERGE INTO T USING S ON T.K = S.K \
     WHEN MATCHED AND S.V > 30 THEN DELETE \
     WHEN MATCHED THEN UPDATE SET V = T.V + S.V, N = S.N \
     WHEN NOT MATCHED AND S.K IS NOT NULL THEN INSERT VALUES (S.K, S.V, S.N)",
    "MERGE INTO T USING (SELECT K + 5 AS K, V FROM S WHERE K < 50) AS P ON T.K = P.K \
     WHEN MATCHED THEN UPDATE SET V = P.V \
     WHEN NOT MATCHED THEN INSERT (K, V) VALUES (P.K, P.V)",
];

/// T's rows after `ALIKE`, sorted as `sorted` sorts them, and the
/// ROW_COUNT of each MERGE: of both kinds of clause, the rows of T RIGHT
/// OUTER JOIN its source, one of each source row, the null key's too
/// [ISO/IEC 9075-2:2023 §23.1 GR2f ii 3].
fn alike_expected() -> (Vec<String>, Vec<String>) {
    let mut found: Vec<String> = (1..=60_i64)
        .map(|k| match k {
            1..=19 => format!("{k},{},t{k}", k * 10),
            20..=24 => format!("{k},{},s{k}", k * 11),
            25..=30 | 41..=54 => format!("{k},{},s{k}", k - 5),
            31..=40 => format!("{k},{},NULL", k - 5),
            _ => format!("{k},{k},s{k}"),
        })
        .collect();
    found.sort();
    (found, vec!["42".to_owned(), "30".to_owned()])
}

/// T's rows after `ALIKE`, of a table `layout` gives, indexed by K as
/// `index` says, read by batches or not as `batches` says; and each
/// MERGE's ROW_COUNT.
fn alike(layout: &str, index: Option<&str>, batches: &str) -> (Vec<String>, Vec<String>) {
    let db = Database::new();
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, V INTEGER, N VARCHAR(8)){layout} CREATE TABLE S (K INTEGER, V INTEGER, N VARCHAR(8))"
    ))
    .unwrap();
    if let Some(kind) = index {
        db.create_index("T_K", "A.T", &["K"], kind).unwrap();
    }
    let a = db.session("A");
    for k in 1..=40 {
        a.run(&format!("INSERT INTO T VALUES ({k}, {}, 't{k}')", k * 10)).unwrap();
    }
    for k in 20..=60 {
        a.run(&format!("INSERT INTO S VALUES ({k}, {k}, 's{k}')")).unwrap();
    }
    a.run("INSERT INTO S VALUES (NULL, 7, 'null')").unwrap();
    a.run(&format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
    let counts = ALIKE
        .iter()
        .map(|sql| {
            a.run(sql).unwrap();
            statement(&a, "ROW_COUNT")
        })
        .collect();
    (sorted(&a, "SELECT K, V, N FROM T"), counts)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6] RULES.md D31 a MERGE writes
/// the same rows, and counts them alike, by batches and row by row, into
/// a table of rows or of columns, with no index on its key or a B-tree or
/// a hash index: deletes, updates, inserts and a source row of a null key,
/// matched by none.
#[test]
fn the_same_rows_by_batches_layouts_and_indexes_14_12() {
    let expected = alike_expected();
    for layout in ["", " USING columnstore"] {
        for index in [None, Some("btree"), Some("hash")] {
            for batches in ["ON", "OFF"] {
                assert_eq!(alike(layout, index, batches), expected, "{layout} {index:?} batches {batches}");
            }
        }
    }
}

/// K_OUT, the fetch target.
fn targets() -> Params {
    Params::new().declare("K_OUT", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null)
}

/// Every K the cursor `name` has still to give, sorted.
fn fetch_rest(s: &Session, p: &mut Params, name: &str) -> Vec<String> {
    let mut seen = Vec::new();
    while s.execute(&format!("FETCH {name} INTO K_OUT"), p).unwrap() == Completion::Success {
        seen.push(p.value("K_OUT").to_string());
    }
    seen.sort();
    seen
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR2, GR3; ISO/IEC 9075:1992
/// §4.21] an INSENSITIVE cursor open on the target sees none of a MERGE's
/// changes, its rows those of when it opened; a cursor of the default
/// sensitivity does not give a row a MERGE deleted, as after a searched
/// DELETE.
#[test]
fn cursors_open_on_the_target_14_12() {
    let db = merged();
    let a = db.session("A");
    a.declare_cursor("DECLARE C INSENSITIVE CURSOR FOR SELECT K FROM T").unwrap();
    let mut p = targets();
    a.execute("OPEN C", &mut p).unwrap();
    a.run(UPSERT).unwrap();
    a.run("MERGE INTO T USING S ON T.K = S.K AND S.K > 2 WHEN MATCHED THEN DELETE").unwrap();
    assert_eq!(fetch_rest(&a, &mut p, "C"), ["1", "2", "3"], "insensitive");
    a.rollback().unwrap();
    a.declare_cursor("DECLARE D CURSOR FOR SELECT K FROM T").unwrap();
    a.execute("OPEN D", &mut p).unwrap();
    a.execute("FETCH D INTO K_OUT", &mut p).unwrap();
    let seen = p.value("K_OUT").to_string();
    a.run(&format!("MERGE INTO T USING (SELECT K FROM T WHERE K <> {seen}) AS O ON T.K = O.K WHEN MATCHED THEN DELETE")).unwrap();
    assert_eq!(a.execute("FETCH D INTO K_OUT", &mut p).unwrap(), Completion::NoData, "the rows deleted not given");
    a.rollback().unwrap();
}
